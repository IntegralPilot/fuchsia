// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Framing and deframing datagrams onto QUIC streams

use super::PeerConnRef;
use crate::labels::NodeId;
use anyhow::{Error, format_err};

/// The type of frame that can be received on a QUIC stream
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameType {
    Hello,
    Data,
    Control,
    Signal,
}

/// Header for one frame of data on a QUIC stream
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameHeader {
    /// Type of the frame
    frame_type: FrameType,
    /// Length of the frame (usize here to avoid casts in client code; this is checked to fit in a
    /// u32 before serialization)
    length: usize,
}

/// Length of the header for a frame.
const FRAME_HEADER_LENGTH: usize = 8;

impl FrameHeader {
    fn to_bytes(&self) -> Result<[u8; FRAME_HEADER_LENGTH], Error> {
        let length = self.length;
        if length > u32::MAX as usize {
            return Err(anyhow::format_err!("Message too long: {}", length));
        }
        let length = length as u32;
        let hdr: u64 = (length as u64)
            | (match self.frame_type {
                FrameType::Hello => 0,
                FrameType::Data => 4,
                FrameType::Control => 5,
                FrameType::Signal => 6,
            } << 32);
        Ok(hdr.to_le_bytes())
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let hdr: &[u8; FRAME_HEADER_LENGTH] = bytes[0..FRAME_HEADER_LENGTH].try_into()?;
        let hdr = u64::from_le_bytes(*hdr);
        let length = (hdr & 0xffff_ffff) as usize;
        let frame_type = match hdr >> 32 {
            0 => FrameType::Hello,
            1 | 2 | 3 => return Err(anyhow::format_err!("Frame with no persistence header")),
            4 => FrameType::Data,
            5 => FrameType::Control,
            6 => FrameType::Signal,
            _ => return Err(anyhow::format_err!("Unknown frame type {}", hdr >> 32)),
        };
        Ok(FrameHeader { frame_type, length })
    }
}

#[derive(Debug)]
pub(crate) struct FramedStreamWriter {
    /// Underlying writer
    writer: circuit::stream::Writer,
    /// The circuit's ID number
    id: u64,
    /// The connection supporting the writer,
    conn: circuit::Connection,
    /// The peer node id
    peer_node_id: NodeId,
}

impl FramedStreamWriter {
    pub fn from_circuit(
        writer: circuit::stream::Writer,
        id: u64,
        conn: circuit::Connection,
        peer_node_id: NodeId,
    ) -> Self {
        Self { writer, id, conn, peer_node_id }
    }

    pub async fn abandon(&mut self) {
        let (_reader, dead_writer) = circuit::stream::stream();
        self.writer = dead_writer;
    }

    pub fn conn(&self) -> PeerConnRef<'_> {
        PeerConnRef::from_circuit(&self.conn, self.peer_node_id)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub async fn send(&mut self, frame_type: FrameType, bytes: &[u8]) -> Result<(), Error> {
        let r = self.send_inner(frame_type, bytes).await;
        if r.is_err() {
            self.abandon().await;
        }
        r
    }

    async fn send_inner(&mut self, frame_type: FrameType, bytes: &[u8]) -> Result<(), Error> {
        let frame_len = bytes.len();
        assert!(frame_len <= 0xffff_ffff);
        let header = FrameHeader { frame_type, length: frame_len }.to_bytes()?;
        log::trace!(header:?; "");
        self.writer.write(header.len(), |buf| {
            buf[..header.len()].copy_from_slice(&header);
            Ok(header.len())
        })?;

        if !bytes.is_empty() {
            self.writer.write(bytes.len(), |buf| {
                buf[..bytes.len()].copy_from_slice(bytes);
                Ok(bytes.len())
            })?;
        }
        Ok(())
    }
}

pub(crate) enum FramedStreamReadResult {
    Frame(FrameType, Vec<u8>),
    Closed(Option<String>),
}

#[derive(Debug)]
pub(crate) struct FramedStreamReader {
    /// The underlying reader,
    reader: circuit::stream::Reader,
    /// The connection supporting th reader.
    conn: circuit::Connection,
    /// Peer node id
    peer_node_id: NodeId,
    /// Current read state
    read_state: ReadState,
    /// Scratch space for reading the frame header
    hdr: [u8; FRAME_HEADER_LENGTH],
}

impl FramedStreamReader {
    pub fn from_circuit(
        reader: circuit::stream::Reader,
        conn: circuit::Connection,
        peer_node_id: NodeId,
    ) -> Self {
        Self {
            reader,
            conn,
            peer_node_id,
            read_state: ReadState::Initial,
            hdr: [0u8; FRAME_HEADER_LENGTH],
        }
    }

    pub(crate) async fn abandon(&mut self) {
        let (dead_reader, _writer) = circuit::stream::stream();
        self.reader = dead_reader;
    }

    pub fn conn(&self) -> PeerConnRef<'_> {
        PeerConnRef::from_circuit(&self.conn, self.peer_node_id)
    }

    pub fn is_initiator(&self) -> bool {
        self.conn.is_client()
    }

    pub(crate) async fn next<'b>(&'b mut self) -> Result<FramedStreamReadResult, Error> {
        if let ReadState::Initial = self.read_state {
            if !read_exact(&self.reader, &mut self.hdr).await? {
                return Ok(FramedStreamReadResult::Closed(self.reader.closed_reason()));
            }
            let hdr = FrameHeader::from_bytes(&self.hdr)?;

            if hdr.length == 0 {
                return Ok(FramedStreamReadResult::Frame(hdr.frame_type, Vec::new()));
            }

            self.read_state = ReadState::GotHeader(hdr);
        }

        let ReadState::GotHeader(hdr) = &self.read_state else {
            unreachable!();
        };

        let mut payload = vec![0; hdr.length];
        payload.resize(hdr.length, 0);
        if !read_exact(&self.reader, &mut payload).await? {
            return Err(format_err!("Unexpected end of stream"));
        }
        let frame_type = hdr.frame_type;
        self.read_state = ReadState::Initial;
        Ok(FramedStreamReadResult::Frame(frame_type, payload))
    }
}

async fn read_exact(reader: &circuit::stream::Reader, buf: &mut [u8]) -> Result<bool, Error> {
    reader
        .read(buf.len(), |input| {
            buf.copy_from_slice(&input[..buf.len()]);
            Ok((true, buf.len()))
        })
        .await
        .or_else(|x| match x {
            circuit::Error::ConnectionClosed(reason) => {
                if let Some(reason) = reason {
                    log::debug!(reason:?; "");
                }
                Ok(false)
            }
            other => Err(other.into()),
        })
}

#[derive(Debug)]
enum ReadState {
    Initial,
    GotHeader(FrameHeader),
}

#[cfg(test)]
mod test {
    use super::*;

    fn roundtrip(h: FrameHeader) {
        assert_eq!(h, FrameHeader::from_bytes(&h.to_bytes().unwrap()).unwrap());
    }

    #[fuchsia::test]
    fn roundtrips() {
        roundtrip(FrameHeader { frame_type: FrameType::Data, length: 0 });
        roundtrip(FrameHeader { frame_type: FrameType::Data, length: u32::MAX as usize });
    }

    #[fuchsia::test]
    fn too_long() {
        FrameHeader { frame_type: FrameType::Data, length: (u32::MAX as usize) + 1 }
            .to_bytes()
            .expect_err("Should fail");
    }

    #[fuchsia::test]
    fn bad_frame_type() {
        assert!(
            format!(
                "{}",
                FrameHeader::from_bytes(&[0, 0, 0, 0, 11, 0, 0, 0]).expect_err("should fail")
            )
            .contains("Unknown frame type 11")
        );
    }
}

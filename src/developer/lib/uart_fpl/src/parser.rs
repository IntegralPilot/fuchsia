// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Streaming frame parser with preamble resynchronization, header integrity verification,
//! and bounded memory allocation.

use zerocopy::FromBytes;
use zerocopy::byteorder::big_endian::U32 as BeU32;

use crate::frame::{
    CHECKSUM_LEN, Frame, FrameType, HEADER_CHECKSUM_OFFSET, HEADER_LEN, MAX_PAYLOAD_SIZE,
    SYNC_WORD, SYNC_WORD_LEN, WireHeader, crc8, crc32,
};

/// Maximum capacity for the internal parser buffer (64 KiB) to prevent unbounded memory growth.
pub(crate) const MAX_BUFFER_CAPACITY: usize = 64 * 1024;

/// Internal buffer compaction threshold in bytes before draining consumed bytes.
const COMPACTION_THRESHOLD_BYTES: usize = 4096;

/// Streaming parser for continuous byte streams from serial interfaces.
#[derive(Debug)]
pub struct FrameParser {
    buffer: Vec<u8>,
    cursor: usize,
    checksum_errors: u64,
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameParser {
    /// Creates a new streaming frame parser.
    pub fn new() -> Self {
        Self { buffer: Vec::new(), cursor: 0, checksum_errors: 0 }
    }

    /// Resets the parser state, clearing all buffered bytes and resetting cursor.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.checksum_errors = 0;
    }

    /// Returns true if there are no unconsumed bytes in the buffer.
    pub fn is_empty(&self) -> bool {
        self.unconsumed_len() == 0
    }

    /// Drains and returns all unconsumed bytes from the internal buffer, resetting the parser.
    pub fn take_unconsumed(&mut self) -> Vec<u8> {
        let remaining = self.buffer[self.cursor..].to_vec();
        self.buffer.clear();
        self.cursor = 0;
        remaining
    }

    /// Returns the cumulative number of CRC32 / header checksum mismatch errors encountered.
    pub fn checksum_errors(&self) -> u64 {
        self.checksum_errors
    }

    /// Returns a slice of the unparsed bytes currently remaining in the buffer.
    #[inline]
    pub fn unconsumed(&self) -> &[u8] {
        &self.buffer[self.cursor..]
    }

    /// Returns the current number of unparsed bytes waiting in the internal stream buffer.
    #[inline]
    pub fn unconsumed_len(&self) -> usize {
        self.buffer.len() - self.cursor
    }

    #[inline]
    fn consume(&mut self, count: usize) {
        self.cursor += count;
        if self.cursor >= self.buffer.len() {
            self.buffer.clear();
            self.cursor = 0;
        }
    }

    /// Appends incoming byte chunks to the internal streaming buffer.
    ///
    /// Memory consumption is strictly bounded to [`MAX_BUFFER_CAPACITY`]; if incoming data
    /// exceeds this limit, the oldest un-parsed bytes are discarded to prevent memory exhaustion.
    pub fn feed(&mut self, data: &[u8]) {
        let unconsumed_len = self.unconsumed_len();
        let total_len = unconsumed_len.saturating_add(data.len());

        // Compact consumed bytes so cursor is at 0 before evaluating overflow trimming.
        if self.cursor > 0
            && (self.cursor >= COMPACTION_THRESHOLD_BYTES
                || self.cursor >= self.buffer.len() / 2
                || self.buffer.len().saturating_add(data.len()) > MAX_BUFFER_CAPACITY)
        {
            self.buffer.drain(..self.cursor);
            self.cursor = 0;
        }

        if total_len > MAX_BUFFER_CAPACITY {
            let overflow = total_len - MAX_BUFFER_CAPACITY;
            let current_len = self.buffer.len();
            if overflow >= current_len {
                self.buffer.clear();
                let data_start = data.len().saturating_sub(MAX_BUFFER_CAPACITY);
                self.buffer.extend_from_slice(&data[data_start..]);
                return;
            } else {
                self.buffer.drain(..overflow);
            }
        }
        self.buffer.extend_from_slice(data);
    }

    /// Parses and extracts the next complete, validated protocol frame from the stream buffer.
    ///
    /// Returns `None` if insufficient bytes are available or if awaiting further stream chunks.
    pub fn next_frame(&mut self) -> Option<Frame> {
        loop {
            let unconsumed = self.unconsumed();
            if unconsumed.len() < SYNC_WORD_LEN {
                return None;
            }

            let Some(sync_pos) = unconsumed.windows(SYNC_WORD_LEN).position(|w| w == SYNC_WORD)
            else {
                let to_consume = if unconsumed.last() == Some(&SYNC_WORD[0]) {
                    unconsumed.len() - 1
                } else {
                    unconsumed.len()
                };
                self.consume(to_consume);
                return None;
            };
            if sync_pos > 0 {
                self.consume(sync_pos);
            }

            let unconsumed = self.unconsumed();
            if unconsumed.len() < HEADER_LEN {
                return None;
            }

            // Verify Header Checksum (HdrChk at byte offset 12)
            let header_fields = &unconsumed[SYNC_WORD_LEN..HEADER_CHECKSUM_OFFSET];
            let received_hdr_chk = unconsumed[HEADER_CHECKSUM_OFFSET];
            let calculated_hdr_chk = crc8(header_fields);

            if received_hdr_chk != calculated_hdr_chk {
                log::debug!(
                    "FrameParser: header checksum mismatch! Calculated: {calculated_hdr_chk:#04x}, received: {received_hdr_chk:#04x}"
                );
                self.checksum_errors += 1;
                self.consume(SYNC_WORD_LEN);
                continue;
            }

            let (wire_header, _) = WireHeader::read_from_prefix(unconsumed)
                .expect("unconsumed length verified >= HEADER_LEN");
            let session_id = wire_header.session_id.get();
            let channel_id = wire_header.channel_id.get();
            let seq = wire_header.seq;
            let frame_type = FrameType::from(wire_header.frame_type);
            let length = wire_header.payload_len.get() as usize;

            if length > MAX_PAYLOAD_SIZE {
                log::debug!("FrameParser: invalid payload length in header: {length}");
                self.consume(SYNC_WORD_LEN);
                continue;
            }

            let total_needed = HEADER_LEN + length + CHECKSUM_LEN;
            if unconsumed.len() < total_needed {
                return None;
            }

            let (received_cksum_be, _) =
                BeU32::read_from_prefix(&unconsumed[HEADER_LEN + length..])
                    .expect("unconsumed length verified >= total_needed");
            let received_cksum = received_cksum_be.get();

            let calculated_cksum = crc32(&unconsumed[SYNC_WORD_LEN..HEADER_LEN + length]);

            if received_cksum == calculated_cksum {
                let is_negotiate =
                    frame_type == FrameType::NegotiateReq || frame_type == FrameType::NegotiateResp;
                if is_negotiate && channel_id != 0 {
                    log::debug!(
                        "FrameParser: dropping handshake frame {frame_type:?} on non-zero channel {channel_id}"
                    );
                    self.consume(total_needed);
                    continue;
                }

                let payload = unconsumed[HEADER_LEN..HEADER_LEN + length].to_vec();
                self.consume(total_needed);
                return Some(Frame { session_id, channel_id, seq, frame_type, payload });
            } else {
                log::debug!(
                    "FrameParser: CRC-32 mismatch for frame type {frame_type:?}, length={length}! Calculated: {calculated_cksum:#010x}, received: {received_cksum:#010x}"
                );
                self.checksum_errors += 1;
                self.consume(SYNC_WORD_LEN);
            }
        }
    }
}

impl Iterator for FrameParser {
    type Item = Frame;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FrameType, PAYLOAD_LEN_OFFSET, encode_frame};

    fn assert_frame_eq(
        frame: &Frame,
        session_id: u32,
        channel_id: u16,
        seq: u8,
        frame_type: FrameType,
        payload: &[u8],
    ) {
        assert_eq!(frame.session_id, session_id);
        assert_eq!(frame.channel_id, channel_id);
        assert_eq!(frame.seq, seq);
        assert_eq!(frame.frame_type, frame_type);
        assert_eq!(frame.payload, payload);
    }

    #[fuchsia::test]
    fn test_frame_parser_happy_path() {
        let payload = b"ping_message";
        let frame = encode_frame(100, 2, 5, FrameType::Data, payload).unwrap();
        let mut parser = FrameParser::new();
        parser.feed(&frame);

        let parsed = parser.next_frame().expect("expected parsed frame");
        assert_frame_eq(&parsed, 100, 2, 5, FrameType::Data, payload);
        assert_eq!(parser.next_frame(), None);
    }

    #[fuchsia::test]
    fn test_frame_parser_partial_reads() {
        let payload = b"fragmented";
        let frame = encode_frame(100, 2, 5, FrameType::Data, payload).unwrap();
        let mut parser = FrameParser::new();

        for chunk in frame.chunks(3) {
            parser.feed(chunk);
        }

        let parsed = parser.next_frame().expect("expected frame after fragments");
        assert_frame_eq(&parsed, 100, 2, 5, FrameType::Data, payload);
        assert_eq!(parser.next_frame(), None);
    }

    #[fuchsia::test]
    fn test_frame_parser_sync_recovery() {
        let payload = b"recovered";
        let valid_frame = encode_frame(1, 1, 0, FrameType::Data, payload).unwrap();

        let mut corrupted_stream = vec![0xFF, 0xAA, 0x00, 0x55, 0x12];
        corrupted_stream.extend_from_slice(&valid_frame);

        let mut parser = FrameParser::new();
        parser.feed(&corrupted_stream);

        let parsed = parser.next_frame().expect("parser must parse valid frame");
        assert_eq!(parsed.session_id, 1);
        assert_eq!(parsed.payload, b"recovered");
    }

    #[fuchsia::test]
    fn test_frame_parser_bad_checksum() {
        let payload = b"corrupted";
        let mut frame = encode_frame(1, 1, 0, FrameType::Data, payload).unwrap();
        let len = frame.len();
        frame[len - 1] ^= 0xFF; // Flip bit in trailing CRC-32

        let mut parser = FrameParser::new();
        parser.feed(&frame);
        assert_eq!(parser.next_frame(), None);
        assert_eq!(parser.checksum_errors(), 1);
    }

    #[fuchsia::test]
    fn test_frame_parser_bad_header_checksum() {
        let payload = b"header_corrupt";
        let mut frame = encode_frame(1, 1, 0, FrameType::Data, payload).unwrap();
        frame[HEADER_CHECKSUM_OFFSET] ^= 0xFF; // Flip bit in header checksum

        let mut parser = FrameParser::new();
        parser.feed(&frame);
        assert_eq!(parser.next_frame(), None);
        assert_eq!(parser.checksum_errors(), 1);
    }

    #[fuchsia::test]
    fn test_frame_parser_checksum_errors_counter() {
        let mut parser = FrameParser::new();
        assert_eq!(parser.checksum_errors(), 0);

        let mut bad_frame = encode_frame(1, 1, 0, FrameType::Data, b"test_crc").unwrap();
        let len = bad_frame.len();
        bad_frame[len - 1] ^= 0xFF;

        parser.feed(&bad_frame);
        assert_eq!(parser.next_frame(), None);
        assert_eq!(parser.checksum_errors(), 1);
    }

    #[fuchsia::test]
    fn test_frame_parser_invalid_length() {
        let mut frame = vec![0xAA, 0x55];
        let mut header_fields = [0u8; 10];
        header_fields[0..4].copy_from_slice(&1u32.to_be_bytes());
        header_fields[4..6].copy_from_slice(&1u16.to_be_bytes());
        header_fields[6] = 0;
        header_fields[7] = u8::from(FrameType::Data);
        header_fields[8..10].copy_from_slice(&2000u16.to_be_bytes()); // Length > 1024

        let hdr_chk = crc8(&header_fields);
        frame.extend_from_slice(&header_fields);
        frame.push(hdr_chk);
        frame.extend_from_slice(&[0u8; 100]);

        let mut parser = FrameParser::new();
        parser.feed(&frame);
        assert_eq!(parser.next_frame(), None);
    }

    #[fuchsia::test]
    fn test_frame_parser_bounded_buffer_capacity() {
        let mut parser = FrameParser::new();
        let massive_chunk = vec![0x42u8; MAX_BUFFER_CAPACITY + 1024];
        parser.feed(&massive_chunk);
        assert!(parser.unconsumed_len() <= MAX_BUFFER_CAPACITY);

        let payload = b"recovered";
        let valid_frame = encode_frame(1, 1, 0, FrameType::Data, payload).unwrap();
        parser.feed(&valid_frame);

        let parsed = parser.next_frame().expect("parser must parse valid frame");
        assert_eq!(parsed.payload, b"recovered");
    }

    #[fuchsia::test]
    fn test_frame_parser_recovers_from_corrupted_length_via_header_checksum() {
        let mut parser = FrameParser::new();

        let mut raw_hdr = [0u8; 10];
        raw_hdr[0..4].copy_from_slice(&1u32.to_be_bytes());
        raw_hdr[4..6].copy_from_slice(&1u16.to_be_bytes());
        raw_hdr[6] = 0;
        raw_hdr[7] = u8::from(FrameType::Data);
        raw_hdr[8..10].copy_from_slice(&10u16.to_be_bytes());
        let hdr_chk = crc8(&raw_hdr);

        let mut corrupted = vec![0xAA, 0x55];
        corrupted.extend_from_slice(&raw_hdr);
        corrupted.push(hdr_chk);
        corrupted.extend_from_slice(b"0123456789");

        // Corrupt length field in header
        corrupted[PAYLOAD_LEN_OFFSET + 1] ^= 0x01;

        parser.feed(&corrupted);
        assert_eq!(parser.next_frame(), None);
        assert_eq!(parser.checksum_errors(), 1);

        // Feed subsequent valid TYPE_DATA frame
        let valid_frame = encode_frame(1, 1, 1, FrameType::Data, b"hello world payload").unwrap();
        parser.feed(&valid_frame);

        let parsed = parser.next_frame().expect("Parser must recover after corrupted header");
        assert_eq!(parsed.session_id, 1);
        assert_eq!(parsed.payload, b"hello world payload");
        assert_eq!(parsed.seq, 1);

        // Subsequent valid ACK frame
        let ack_frame = encode_frame(1, 1, 1, FrameType::Ack, &[]).unwrap();
        parser.feed(&ack_frame);
        let parsed_ack = parser.next_frame().expect("Subsequent ACK must parse");
        assert_eq!(parsed_ack.frame_type, FrameType::Ack);

        // Subsequent valid CLOSE frame
        let close_frame = encode_frame(1, 1, 2, FrameType::Close, &[]).unwrap();
        parser.feed(&close_frame);
        let parsed_close = parser.next_frame().expect("Subsequent CLOSE must parse");
        assert_eq!(parsed_close.frame_type, FrameType::Close);
    }

    #[fuchsia::test]
    fn test_negotiate_req_dropped_on_non_zero_channel() {
        let mut parser = FrameParser::new();
        let invalid_req = encode_frame(1, 1, 0, FrameType::NegotiateReq, b"invalid").unwrap();
        parser.feed(&invalid_req);
        assert_eq!(parser.next_frame(), None);

        let valid_req = encode_frame(1, 0, 0, FrameType::NegotiateReq, b"valid").unwrap();
        parser.feed(&valid_req);
        let parsed = parser.next_frame().expect("NegotiateReq on channel 0 accepted");
        assert_eq!(parsed.channel_id, 0);
        assert_eq!(parsed.frame_type, FrameType::NegotiateReq);
        assert_eq!(parsed.payload, b"valid");
    }

    #[fuchsia::test]
    fn test_frame_parser_state_management() {
        let mut parser = FrameParser::new();
        assert!(parser.is_empty());
        assert_eq!(parser.unconsumed_len(), 0);

        parser.feed(b"partial_stream_data");
        assert!(!parser.is_empty());
        assert_eq!(parser.unconsumed_len(), 19);

        let unconsumed = parser.take_unconsumed();
        assert_eq!(unconsumed, b"partial_stream_data");
        assert!(parser.is_empty());
        assert_eq!(parser.unconsumed_len(), 0);

        parser.feed(b"new_data");
        assert!(!parser.is_empty());
        parser.reset();
        assert!(parser.is_empty());
        assert_eq!(parser.unconsumed_len(), 0);
    }

    #[fuchsia::test]
    fn test_frame_parser_iterator() {
        let frame1 = encode_frame(1, 1, 0, FrameType::Data, b"first").unwrap();
        let frame2 = encode_frame(1, 1, 1, FrameType::Data, b"second").unwrap();

        let mut parser = FrameParser::new();
        parser.feed(&frame1);
        parser.feed(&frame2);

        let frames: Vec<Frame> = parser.collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].payload, b"first");
        assert_eq!(frames[1].payload, b"second");
    }

    #[fuchsia::test]
    fn test_differential_chunking() {
        let mut stream = Vec::new();
        for i in 0..50u8 {
            let payload = format!("chunk-msg-{}", i).into_bytes();
            let frame = encode_frame(42, 1, i, FrameType::Data, &payload).unwrap();
            stream.extend_from_slice(&frame);
            if i % 5 == 0 {
                stream.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
            }
        }

        let mut parser_mono = FrameParser::new();
        parser_mono.feed(&stream);
        let mono_frames: Vec<Frame> = parser_mono.collect();

        let mut parser_frag = FrameParser::new();
        let mut offset = 0;
        let chunk_sizes = [1, 3, 7, 2, 5, 4, 11, 1];
        let mut size_idx = 0;
        while offset < stream.len() {
            let sz = chunk_sizes[size_idx % chunk_sizes.len()];
            size_idx += 1;
            let end = std::cmp::min(offset + sz, stream.len());
            parser_frag.feed(&stream[offset..end]);
            offset = end;
        }
        let frag_frames: Vec<Frame> = parser_frag.collect();

        assert_eq!(mono_frames.len(), 50);
        assert_eq!(mono_frames, frag_frames);
    }
}

// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::util::convert_log_err;
use bytes::Bytes;
use core::iter::Iterator;
use sparse::reader::SparseReader;
use std::cmp::min;
use std::io::{Read, Seek, SeekFrom};
use std::num::NonZeroU64;
use std::range::Range;
use zerocopy::{Immutable, IntoBytes};

struct BytesBox<T>(Box<[T]>);

impl<T> AsRef<[u8]> for BytesBox<T>
where
    T: IntoBytes,
    [T]: Immutable,
{
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

// Streaming flash is an optional protocol used to reduce bandwidth,
// break up images that are too large for the device download buffer,
// and leverage device async I/O to speed partition flashing.
//
// Terminology used for the data structures in this module:
// * A `StreamCommandList` is a list of commands that describe how to write a
//   partition image to the target partition. These commands are independent and
//   idempotent.
// * A `StreamCommand` is a range within the target partition and a description
//   of what the device should do within that range to generate data to write.
// * A `StreamOp` is a description of the bytes defining the owning `StreamCommand`:
//   either a single repeated 32 bit integer, or arbitrary data with a CRC32 checksum.
//
// `Chunk` and `Payload` correspond to `StreamCommand` and `StreamOp` but are
// used as intermediate structures while processing the partition image into operations.
#[derive(Debug, PartialEq)]
pub enum StreamOp {
    Flash { data: Bytes, crc32: u32 },
    Fill { val: u32, length_bytes: u64 },
}

impl StreamOp {
    fn from_data(data: Bytes) -> Self {
        let crc32 = crc32fast::hash(&data);
        Self::Flash { data, crc32 }
    }

    fn from_fill(val: u32, length_bytes: u64) -> Self {
        Self::Fill { val, length_bytes }
    }
}

#[derive(Debug, PartialEq)]
pub struct StreamCommand {
    pub offset_bytes: u64,
    pub op: StreamOp,
}

impl StreamCommand {
    fn from_fill(val: u32, offset_bytes: u64, length_bytes: u64) -> Self {
        Self { offset_bytes, op: StreamOp::from_fill(val, length_bytes) }
    }

    fn from_data(data: Bytes, offset_bytes: u64) -> Self {
        Self { offset_bytes, op: StreamOp::from_data(data) }
    }
}

struct Chunk {
    range: Range<u64>,
    payload: Payload,
}

struct ChunkError;

impl TryFrom<sparse::Chunk> for Chunk {
    type Error = ChunkError;

    fn try_from(val: sparse::Chunk) -> Result<Self, Self::Error> {
        match val {
            sparse::Chunk::Raw { start, size } => {
                Ok(Self { range: Range { start, end: start + size }, payload: Payload::Flash })
            }
            sparse::Chunk::Fill { start, size, value } => Ok(Self {
                range: Range { start, end: start + size },
                payload: Payload::Fill(value),
            }),
            _ => Err(ChunkError),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Payload {
    Fill(u32),
    Flash,
}

impl Payload {
    fn from_segment(segment: &[u8]) -> Self {
        // Safety:
        // * All bit patterns are valid u32
        // * Segment _should_ be u32 aligned and sized.
        let (_, data, _) = unsafe { segment.align_to::<u32>() };
        assert_eq!(
            data.len() * std::mem::size_of::<u32>(),
            segment.len(),
            "Data segment length violation"
        );
        // If data.iter().next().is_none(),
        // we get a 0-length Fill data with a payload of 0,
        // which is a no-op.
        // Technically we could filter these no-op commands,
        // but even though they are syntactically possible in this context
        // they aren't possible due to the way that chunk_range is constructed.
        let val = data.iter().next().unwrap_or(&0);
        if data.iter().all(|v| v == val) { Self::Fill(*val) } else { Self::Flash }
    }
}

fn range_length<T: std::ops::Sub>(range: Range<T>) -> <T as std::ops::Sub>::Output {
    let Range { start, end } = range;
    end - start
}

impl Chunk {
    fn from_payload(range: Range<u64>, payload: Payload) -> Self {
        Self { range, payload }
    }

    fn from_data(range: Range<u64>, data: &[u8]) -> Self {
        Self::from_payload(range, Payload::from_segment(&data[convert_range::<_, usize>(range)]))
    }

    fn offset(&self) -> u64 {
        self.range.start
    }

    fn len_bytes(&self) -> u64 {
        range_length(self.range)
    }

    fn with_extension(self, length: u64) -> Self {
        let Self { range: Range { start, end }, payload } = self;
        Self { range: Range { start, end: end + length }, payload }
    }
}

fn convert_range<T, U>(r: Range<T>) -> Range<U>
where
    U: TryFrom<T>,
    <U as TryFrom<T>>::Error: std::error::Error,
{
    Range { start: convert_log_err(r.start).unwrap(), end: convert_log_err(r.end).unwrap() }
}

// Process the segment into the work-in-progress chunk.
// The current chunk is either expanded to include the new segment OR
// is wrapped up into a completed command, pushed to the command vector,
// and a new work-in-progress chunk is returned.
fn process_segment(
    data: &Bytes,
    segment_range: Range<u64>,
    max_download_bytes: u64,
    commands: &mut Vec<StreamCommand>,
    chunk: Chunk,
) -> Chunk {
    use Payload::*;

    let segment_len_bytes = range_length(segment_range);
    let new_payload =
        Payload::from_segment(&data.as_ref()[convert_range::<_, usize>(segment_range)]);
    match (chunk.payload, new_payload) {
        // New payload is a flash or a fill with a different value
        (p1 @ Fill(val), p2) if p1 != p2 => {
            commands.push(StreamCommand::from_fill(val, chunk.offset(), chunk.len_bytes()));
            Chunk::from_payload(segment_range, new_payload)
        }
        (Flash, Fill(_)) => {
            commands.push(StreamCommand::from_data(
                data.slice(convert_range::<_, usize>(chunk.range)),
                chunk.offset(),
            ));
            Chunk::from_payload(segment_range, new_payload)
        }
        (Flash, Flash)
            if chunk.len_bytes().checked_add(segment_len_bytes).expect("Integer overflow")
                > max_download_bytes =>
        {
            // Can't have a Flash segment longer than max_download_bytes.
            // Even though the following chunk is also raw data, we need to
            // split it up into multiple commands.
            commands.push(StreamCommand::from_data(
                data.slice(convert_range::<_, usize>(chunk.range)),
                chunk.offset(),
            ));
            Chunk::from_payload(segment_range, new_payload)
        }
        // Matching fill or under max size flash
        _ => chunk.with_extension(segment_len_bytes),
    }
}

// Sparse images are enough similar to streaming commands to be converted
// on the fly by the host.
//
// For the most part sparse images can be converted directly to a sequence of
// StreamCommand structs, but there's a catch: sparse files can be very large.
// To prevent excessive memory usage/OoM on the host, we construct an iterator
// that dynamically generates and yields StreamCommands from a sparse image, never
// allocating a buffer larger than the max download size at once.
pub struct SparseStreamIterator<R> {
    // The size of the image written to the target partition.
    expanded_size: u64,
    reader: R,
    chunks: std::vec::IntoIter<(Chunk, Option<u64>)>,
    // Used for splitting sparse raw chunks.
    // If Some, stores a tuple of bytes remaining in the chunk
    // and the offset in the expanded image.
    remaining_bytes_and_offset: Option<(NonZeroU64, u64)>,
    max_download_bytes: u64,
}

impl<R: Read + Seek> SparseStreamIterator<R> {
    pub fn new(max_download_bytes: u64, sparse_reader: SparseReader<R>) -> Self {
        let (reader, _, expanded_size, chunks, _) = sparse_reader.destruct();
        let chunks = chunks
            .into_iter()
            .filter_map(|(chunk, idx): (sparse::Chunk, Option<u64>)| {
                chunk.try_into().ok().map(|c| (c, idx))
            })
            .collect::<Vec<_>>()
            .into_iter();
        Self { expanded_size, reader, chunks, remaining_bytes_and_offset: None, max_download_bytes }
    }

    pub fn get_expanded_size(&self) -> u64 {
        self.expanded_size
    }

    // Generate a stream flash command to handle a max_download_bytes sized chunk of
    // the current Flash segment or the remainder of the segment, whichever is smaller.
    // Stores metadata if there is any remaining data that needs to be handled.
    //
    // It is the caller's responsibility to call self.reader.seek() at the start of a
    // new Flash command. The reader cursor is preserved across calls,
    // so handling consecutive subchunks works out fine.
    fn handle_subchunk(&mut self, bytes_remaining: u64, offset: u64) -> StreamCommand {
        let buf_len = min(self.max_download_bytes, bytes_remaining);
        let mut buf = vec![0u8; buf_len.try_into().unwrap()];
        self.reader.read_exact(&mut buf).unwrap();

        self.remaining_bytes_and_offset =
            NonZeroU64::new(bytes_remaining - buf_len).map(|s| (s, offset + buf_len));

        StreamCommand::from_data(Bytes::from_owner(buf.into_boxed_slice()), offset)
    }
}

impl<R: Read + Seek> Iterator for SparseStreamIterator<R> {
    type Item = StreamCommand;

    // We need to handle exactly one case specially:
    // if a Raw chunk is larger than the max download size, then it gets split.
    // Fills are converted directly, CRC and DONTCARE are filtered, and it's not
    // a problem if we generate Flash commands that are smaller than max_download_bytes.
    // It's more trouble than it's worth to merge Flash or Fill commands.
    fn next(&mut self) -> Option<Self::Item> {
        if let Some((remainder, offset)) = self.remaining_bytes_and_offset.take() {
            return Some(self.handle_subchunk(remainder.into(), offset));
        }
        let (chunk, reader_offset) = self.chunks.next()?;
        let cmd = match chunk.payload {
            Payload::Fill(val) => StreamCommand::from_fill(val, chunk.offset(), chunk.len_bytes()),
            Payload::Flash => {
                let _ = self.reader.seek(SeekFrom::Start(reader_offset.unwrap())).unwrap();
                self.handle_subchunk(chunk.len_bytes(), chunk.offset())
            }
        };
        Some(cmd)
    }
}

/// Generate a list of streaming flash commands.
/// Takes a partition image, the max download size the device supports,
/// the streaming segment size, and the partition offset from the start of the device.
///
/// Note: `data` is a boxed slice of u32, but all other parameters are denominated in bytes.
/// The passed data is denominated in u32 to allow aligned value checking for making
/// Fill commands, which describe a repeated 32-bit integer.
/// Other operations on the data treats it as a byte array due to limitations of the
/// Bytes structure.
pub fn generate_command_list(
    data: Box<[u32]>,
    max_download_bytes: u64,
    segment_size_bytes: u64,
    partition_start_byte: u64,
) -> Vec<StreamCommand> {
    let data = Bytes::from_owner(BytesBox(data));
    let prefix_bytes =
        partition_start_byte.next_multiple_of(segment_size_bytes) - partition_start_byte;

    let data_len: u64 = convert_log_err(data.len()).unwrap();
    let prefix_bytes = if prefix_bytes == 0 { segment_size_bytes } else { prefix_bytes };
    // The first chunk primes the process_segment sequence
    // and handles a possible unaligned prefix.
    let mut chunk = Chunk::from_data(Range { start: 0, end: min(prefix_bytes, data_len) }, &data);

    let mut commands = vec![];
    for segment_range in (prefix_bytes..data_len)
        .step_by(convert_log_err(segment_size_bytes).unwrap())
        .map(|start| Range { start, end: start + min(segment_size_bytes, data_len - start) })
    {
        chunk = process_segment(&data, segment_range, max_download_bytes, &mut commands, chunk);
    }

    let final_command = match chunk.payload {
        Payload::Fill(val) => StreamCommand::from_fill(val, chunk.offset(), chunk.len_bytes()),
        Payload::Flash => StreamCommand::from_data(
            data.slice(convert_range::<_, usize>(chunk.range)),
            chunk.offset(),
        ),
    };
    commands.push(final_command);

    commands
}

#[cfg(test)]
mod test {
    use core::ops::{Deref, DerefMut};
    use std::io::Write;

    use sparse::builder::{DataSource, SparseImageBuilder};

    use super::*;
    use crate::util::{U32_SIZE, multi_chain};

    #[fuchsia::test()]
    fn test_stream_command_generation_basic() {
        const SEGMENT_SIZE_WORDS: u32 = 1024;
        let flash_data = 0u32..SEGMENT_SIZE_WORDS;
        let fill_data = std::iter::repeat(0u32).take(SEGMENT_SIZE_WORDS as usize);
        let data = multi_chain!(
            flash_data.clone(),
            fill_data.clone(),
            fill_data.clone(),
            fill_data.clone(),
            flash_data.clone(),
        )
        .collect::<Vec<_>>()
        .into_boxed_slice();

        let command_list =
            generate_command_list(data, 1 << 32, u64::from(SEGMENT_SIZE_WORDS) * U32_SIZE, 0);

        let cmd_data =
            Bytes::from_owner(BytesBox(Vec::from_iter(flash_data.clone()).into_boxed_slice()));
        assert_eq!(
            command_list,
            [
                StreamCommand::from_data(cmd_data.clone(), 0),
                StreamCommand::from_fill(0, 4096, 3 * 4096),
                StreamCommand::from_data(cmd_data.clone(), 16384),
            ]
        );
    }

    #[fuchsia::test()]
    fn test_stream_command_generation_different_fill() {
        const SEGMENT_SIZE_WORDS: u32 = 1024;
        let fill_zero = std::iter::repeat(0u32).take(SEGMENT_SIZE_WORDS as usize);
        let fill_one = std::iter::repeat(1u32).take(SEGMENT_SIZE_WORDS as usize);

        let data = multi_chain!(
            fill_zero.clone(),
            fill_zero.clone(),
            fill_one.clone(),
            fill_one.clone(),
            fill_zero.clone(),
            fill_one.clone(),
            fill_one.clone(),
            fill_one.clone(),
            fill_one.clone(),
            fill_zero.clone(),
            fill_zero.clone(),
        )
        .collect::<Vec<_>>()
        .into_boxed_slice();

        let command_list =
            generate_command_list(data, 1 << 32, u64::from(SEGMENT_SIZE_WORDS) * U32_SIZE, 0);

        assert_eq!(
            command_list,
            [
                StreamCommand::from_fill(0, 0, 8192),
                StreamCommand::from_fill(1, 8192, 8192),
                StreamCommand::from_fill(0, 16384, 4096),
                StreamCommand::from_fill(1, 20480, 16384),
                StreamCommand::from_fill(0, 36864, 8192),
            ]
        );
    }

    #[fuchsia::test()]
    fn test_stream_command_generation_split_flash() {
        const SEGMENT_SIZE_WORDS: u32 = 1024;

        let flash_data = 0..SEGMENT_SIZE_WORDS;
        let data = multi_chain!(
            flash_data.clone(),
            flash_data.clone(),
            flash_data.clone(),
            flash_data.clone(),
            flash_data.clone(),
            flash_data.clone(),
            flash_data.clone(),
            flash_data.clone(),
        )
        .collect::<Vec<_>>()
        .into_boxed_slice();

        let command_list = generate_command_list(
            data,
            u64::from(SEGMENT_SIZE_WORDS) * U32_SIZE * 2,
            u64::from(SEGMENT_SIZE_WORDS) * U32_SIZE,
            0,
        );

        let command_data = Bytes::from_owner(BytesBox(
            multi_chain!(flash_data.clone(), flash_data.clone())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        ));
        assert_eq!(
            command_list,
            [
                StreamCommand::from_data(command_data.clone(), 0),
                StreamCommand::from_data(command_data.clone(), 8192),
                StreamCommand::from_data(command_data.clone(), 16384),
                StreamCommand::from_data(command_data.clone(), 24576),
            ]
        );
    }

    #[fuchsia::test()]
    fn test_stream_command_generation_unaligned_prefix() {
        const SEGMENT_SIZE_WORDS: u32 = 1024;

        let flash_data = 0..SEGMENT_SIZE_WORDS;
        let data = multi_chain!(
            0..(SEGMENT_SIZE_WORDS / 2),
            flash_data.clone(),
            flash_data.clone(),
            flash_data.clone(),
            flash_data.clone(),
        )
        .collect::<Vec<_>>();

        let command_list = generate_command_list(
            data.clone().into_boxed_slice(),
            1 << 32,
            u64::from(SEGMENT_SIZE_WORDS) * U32_SIZE,
            u64::from(SEGMENT_SIZE_WORDS) * U32_SIZE / 2,
        );

        assert_eq!(
            command_list,
            [StreamCommand::from_data(
                Bytes::from_owner(BytesBox(data.clone().into_boxed_slice())),
                0
            )]
        );
    }

    #[fuchsia::test()]
    fn test_stream_command_generation_trailing_suffix() {
        const SEGMENT_SIZE_WORDS: u32 = 1024;

        let fill_zero = std::iter::repeat(0u32).take(SEGMENT_SIZE_WORDS as usize);
        let flash_data = 0..(SEGMENT_SIZE_WORDS / 2);
        let data = multi_chain!(
            fill_zero.clone(),
            fill_zero.clone(),
            fill_zero.clone(),
            fill_zero.clone(),
            flash_data.clone(),
        )
        .collect::<Vec<_>>()
        .into_boxed_slice();

        let command_list =
            generate_command_list(data, 1 << 32, u64::from(SEGMENT_SIZE_WORDS) * U32_SIZE, 0);

        assert_eq!(
            command_list,
            [
                StreamCommand::from_fill(0, 0, 16384),
                StreamCommand::from_data(
                    Bytes::from_owner(BytesBox(flash_data.collect::<Vec<_>>().into_boxed_slice())),
                    16384
                )
            ]
        );
    }

    #[fuchsia::test()]
    fn test_stream_command_generation_tiny_image() {
        const SEGMENT_SIZE_WORDS: u32 = 1024;

        let data = vec![0u32; 8].into_boxed_slice();
        let command_list =
            generate_command_list(data, 1 << 32, u64::from(SEGMENT_SIZE_WORDS) * U32_SIZE, 0);

        assert_eq!(command_list, [StreamCommand::from_fill(0, 0, 32)]);
    }

    struct MemFile<B> {
        data: B,
        cursor: usize,
    }

    impl<B> MemFile<B> {
        fn new(data: B) -> Self {
            Self { data, cursor: 0 }
        }
    }

    impl<B: Deref<Target = [u8]>> Seek for MemFile<B> {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            let new_cursor = match pos {
                SeekFrom::End(v) => {
                    i64::try_from(self.data.len()).map(|e| e + v).and_then(u64::try_from).unwrap()
                }
                SeekFrom::Start(v) => v,
                SeekFrom::Current(v) => {
                    i64::try_from(self.cursor).map(|c| c + v).and_then(u64::try_from).unwrap()
                }
            };
            self.cursor = min(new_cursor.try_into().unwrap(), self.data.len());
            Ok(self.cursor.try_into().unwrap())
        }
    }

    impl<B: Deref<Target = [u8]>> Read for MemFile<B> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if let Some(remaining_data) = self.data.len().checked_sub(self.cursor) {
                let data_length = min(buf.len(), remaining_data);
                (&mut buf[..data_length])
                    .copy_from_slice(&self.data[self.cursor..self.cursor + data_length]);
                self.cursor += data_length;
                Ok(data_length)
            } else {
                Ok(0)
            }
        }
    }

    impl<B: DerefMut<Target = [u8]>> Write for MemFile<B> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if let Some(remaining_buffer) = self.data.len().checked_sub(self.cursor) {
                let data_length = min(buf.len(), remaining_buffer);
                (&mut self.data[self.cursor..self.cursor + data_length])
                    .copy_from_slice(&buf[..data_length]);
                self.cursor += data_length;
                Ok(data_length)
            } else {
                Ok(0)
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[fuchsia::test()]
    fn test_stream_command_generation_sparse_basic() {
        let raw = (0u8..255).cycle().take(4096);
        let data = multi_chain!(raw.clone(), raw.clone(), raw.clone(), raw.clone())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let short_data = raw.clone().collect::<Vec<_>>().into_boxed_slice();

        let builder = SparseImageBuilder::new()
            // Basic raw data
            .add_source(DataSource::Buffer(data.clone()))
            // Skip/DONTCARE gets filtered
            .add_source(DataSource::Skip(16384))
            // Basic fill
            .add_source(DataSource::Fill(0, 4096))
            // Second basic raw data
            .add_source(DataSource::Buffer(data.clone()))
            // Spacer
            .add_source(DataSource::Fill(0, 4096))
            // Raw data smaller than max download
            .add_source(DataSource::Buffer(short_data.clone()));
        let mut sparse_image_file =
            MemFile::new(vec![0u8; builder.built_size().try_into().unwrap()]);
        builder.build(&mut sparse_image_file).unwrap();
        sparse_image_file.seek(SeekFrom::Start(0)).unwrap();

        let sparse_reader = SparseReader::new(sparse_image_file).unwrap();
        let commands = SparseStreamIterator::new(8192, sparse_reader).collect::<Vec<_>>();

        let data_helper =
            Bytes::from_owner(data[0..8192].iter().copied().collect::<Vec<_>>().into_boxed_slice());
        assert_eq!(
            commands,
            [
                StreamCommand::from_data(data_helper.clone(), 0),
                StreamCommand::from_data(data_helper.clone(), 8192),
                StreamCommand::from_fill(0, 32768, 16384),
                StreamCommand::from_data(data_helper.clone(), 49152),
                StreamCommand::from_data(data_helper.clone(), 57344),
                StreamCommand::from_fill(0, 65536, 16384),
                StreamCommand::from_data(Bytes::from_owner(short_data), 81920),
            ]
        );
    }
}

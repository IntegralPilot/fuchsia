// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::format::{CHUNK_HEADER_SIZE, SPARSE_HEADER_SIZE};
use crate::{
    BLK_SIZE, Chunk, NO_SOURCE, SparseDataType, SparseError, SparseHeader, UnalignedSource,
};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::ops::Range;

/// Input data for a SparseImageBuilder.
pub enum DataSource {
    /// Heap allocated buffer.
    Buffer(Box<[u8]>),
    /// Read `size` bytes from `reader`.
    Reader { reader: Box<dyn Read>, size: u64 },
    /// Skips this many bytes.
    Skip(u64),
    /// Repeats the given u32, this many times.
    Fill(u32, u64),
    #[cfg(target_os = "fuchsia")]
    /// Read `size` bytes from `vmo` at `offset`.
    Vmo { vmo: zx::Vmo, size: u64, offset: u64 },
}

impl DataSource {
    /// The size of the source in bytes.
    fn data_size(&self) -> u64 {
        match &self {
            DataSource::Buffer(buf) => buf.len() as u64,
            DataSource::Reader { reader: _, size } => *size,
            DataSource::Skip(size) => *size,
            DataSource::Fill(_, count) => *count * std::mem::size_of::<u32>() as u64,
            #[cfg(target_os = "fuchsia")]
            DataSource::Vmo { vmo: _, size, offset: _ } => *size,
        }
    }
}

/// Builds sparse image files from a set of input DataSources.
pub struct SparseImageBuilder {
    block_size: u32,
    sources: Vec<DataSource>,

    // The `total_sz` field in the chunk's header is 32 bits and includes the 12 bytes for the
    // header itself. The chunk size must be a multiple of the block size so the maximum chunk size
    // is 4GiB minus the block size provided the block size is greater than or equal to 12. This
    // field could be derived from `block_size` but is stored separately so it can be set in tests
    // to avoid creating 4GiB data sources.
    max_chunk_size: u32,
}

impl SparseImageBuilder {
    pub fn new() -> Self {
        Self { block_size: BLK_SIZE, sources: vec![], max_chunk_size: u32::MAX - BLK_SIZE + 1 }
    }

    pub fn set_block_size(mut self, block_size: u32) -> Self {
        assert!(
            block_size >= CHUNK_HEADER_SIZE,
            "The block size must be greater than {}",
            CHUNK_HEADER_SIZE
        );
        self.max_chunk_size = u32::MAX - block_size + 1;
        self.block_size = block_size;
        self
    }

    /// Adds the given data `source` to the image. `source` may be encoded as one or more chunks.
    pub fn add_source(mut self, source: DataSource) -> Self {
        self.sources.push(source);
        self
    }

    /// Calculates total amount of space required to build the sparse image given the current set of
    /// data sources.
    pub fn built_size(&self) -> u64 {
        let mut built_size = SPARSE_HEADER_SIZE as u64;
        for source in &self.sources {
            for size in ChunkedRange::new(0..source.data_size(), self.max_chunk_size) {
                let size = size as u64;
                // We only need the encoded size for each chunk once it complies with max chunk size
                // so we can ignore start offset.
                let start = 0;
                let chunk = match &source {
                    DataSource::Buffer(..) => Chunk::Raw { start, size },
                    DataSource::Reader { .. } => Chunk::Raw { start, size },
                    DataSource::Skip(..) => Chunk::DontCare { start, size },
                    DataSource::Fill(..) => Chunk::Fill { start, size, value: 0 },
                    #[cfg(target_os = "fuchsia")]
                    DataSource::Vmo { .. } => Chunk::Raw { start, size },
                };
                built_size += chunk.chunk_data_len() as u64;
            }
        }
        built_size
    }

    pub fn build<W: Write + Seek>(self, output: &mut W) -> Result<(), SparseError> {
        // We'll fill the header in later.
        output.seek(SeekFrom::Start(SPARSE_HEADER_SIZE as u64))?;
        let mut chunk_writer = ChunkWriter::new(self.block_size, output);
        for source in self.sources {
            match source {
                DataSource::Buffer(buf) => {
                    if buf.len() % self.block_size as usize != 0 {
                        return Err(SparseError::UnalignedDataSource(UnalignedSource::Buffer(
                            buf.len(),
                        )));
                    }
                    for slice in buf.chunks(self.max_chunk_size as usize) {
                        chunk_writer
                            .write_raw_chunk(slice.len().try_into().unwrap(), Cursor::new(slice))?;
                    }
                }
                DataSource::Reader { mut reader, size } => {
                    if size % self.block_size as u64 != 0 {
                        return Err(SparseError::UnalignedDataSource(UnalignedSource::Reader(
                            size,
                        )));
                    }
                    for size in ChunkedRange::new(0..size, self.max_chunk_size) {
                        chunk_writer.write_raw_chunk(size, (&mut reader).take(size as u64))?;
                    }
                }
                DataSource::Skip(size) => {
                    if size % self.block_size as u64 != 0 {
                        return Err(SparseError::UnalignedDataSource(UnalignedSource::Skip(size)));
                    }
                    for size in ChunkedRange::new(0..size, self.max_chunk_size) {
                        chunk_writer.write_dont_care_chunk(size)?;
                    }
                }
                DataSource::Fill(value, count) => {
                    let size = count * std::mem::size_of::<u32>() as u64;
                    if size % self.block_size as u64 != 0 {
                        return Err(SparseError::UnalignedDataSource(UnalignedSource::Fill(size)));
                    }
                    for size in ChunkedRange::new(0..size, self.max_chunk_size) {
                        chunk_writer.write_fill_chunk(size, value)?;
                    }
                }
                #[cfg(target_os = "fuchsia")]
                DataSource::Vmo { vmo, size, mut offset } => {
                    if size % self.block_size as u64 != 0 {
                        return Err(SparseError::UnalignedDataSource(UnalignedSource::Vmo(size)));
                    }
                    let mut buffer =
                        vec![0; std::cmp::min(size as usize, self.max_chunk_size as usize)];
                    for size in ChunkedRange::new(0..size, self.max_chunk_size) {
                        let buffer = &mut buffer[0..size as usize];
                        vmo.read(buffer, offset).unwrap();
                        chunk_writer.write_raw_chunk(size, Cursor::new(buffer))?;
                        offset += size as u64;
                    }
                }
            };
        }

        let ChunkWriter { num_blocks, num_chunks, .. } = chunk_writer;
        output.seek(SeekFrom::Start(0))?;
        let header = SparseHeader::new(self.block_size, num_blocks, num_chunks);
        bincode::serialize_into(&mut *output, &header)
            .map_err(|e| SparseError::Serialize { ty: SparseDataType::Header, source: e })?;

        output.flush()?;
        Ok(())
    }

    #[cfg(target_os = "fuchsia")]
    pub fn build_vmo(self) -> Result<zx::Vmo, SparseError> {
        let vmo = zx::Vmo::create(self.built_size())?;
        let mut stream = zx::Stream::create(zx::StreamOptions::MODE_WRITE, &vmo, 0)?;
        self.build(&mut stream)?;
        Ok(vmo)
    }
}

struct ChunkWriter<'a, W> {
    block_size: u32,
    current_offset: u64,
    num_chunks: u32,
    num_blocks: u32,
    writer: &'a mut W,
}

impl<'a, W: Write> ChunkWriter<'a, W> {
    fn new(block_size: u32, writer: &'a mut W) -> Self {
        Self { block_size, current_offset: 0, num_chunks: 0, num_blocks: 0, writer }
    }

    fn write_chunk_impl<R: Read>(
        &mut self,
        chunk: Chunk,
        source: Option<&mut R>,
    ) -> Result<(), SparseError> {
        chunk.write(source, &mut self.writer, self.block_size)?;
        self.num_blocks = self
            .num_blocks
            .checked_add(chunk.output_blocks(self.block_size))
            .ok_or(SparseError::TooManyBlocks)?;
        // The number of blocks and chunks are both a u32. Each chunk contains at least 1 block so
        // the number of blocks will overflow above before the number of chunks.
        self.num_chunks += 1;
        self.current_offset += chunk.output_size() as u64;
        Ok(())
    }

    fn write_raw_chunk<R: Read>(&mut self, size: u32, mut source: R) -> Result<(), SparseError> {
        self.write_chunk_impl(
            Chunk::Raw { start: self.current_offset, size: size.into() },
            Some(&mut source),
        )
    }

    fn write_dont_care_chunk(&mut self, size: u32) -> Result<(), SparseError> {
        self.write_chunk_impl(
            Chunk::DontCare { start: self.current_offset, size: size.into() },
            NO_SOURCE,
        )
    }

    fn write_fill_chunk(&mut self, size: u32, value: u32) -> Result<(), SparseError> {
        self.write_chunk_impl(
            Chunk::Fill { start: self.current_offset, size: size.into(), value },
            NO_SOURCE,
        )
    }
}

/// An iterator that yields `max_chunk_size` `(range.end - range.start) / max_chunk_size` times
/// followed by `(range.end - range.start) % max_chunk_size` if it's none zero.
///
/// # Examples
/// ```
/// assert_eq!(ChunkedRange::new(0..10, 5).collect::<Vec<_>>(), vec![5, 5]);
/// assert_eq!(ChunkedRange::new(0..13, 5).collect::<Vec<_>>(), vec![5, 5, 3]);
/// ```
struct ChunkedRange {
    range: Range<u64>,
    max_chunk_size: u32,
}

impl ChunkedRange {
    fn new(range: Range<u64>, max_chunk_size: u32) -> Self {
        Self { range, max_chunk_size }
    }
}

impl Iterator for ChunkedRange {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        let size = self.range.end - self.range.start;
        if size == 0 {
            None
        } else if size >= self.max_chunk_size as u64 {
            self.range.start += self.max_chunk_size as u64;
            Some(self.max_chunk_size)
        } else {
            self.range.start = self.range.end;
            Some(size as u32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::CHUNK_HEADER_SIZE;
    use crate::reader::SparseReader;

    #[test]
    fn test_chunked_range() {
        assert_eq!(&ChunkedRange::new(0..0, 32).collect::<Vec<_>>(), &[]);
        assert_eq!(&ChunkedRange::new(0..10, 32).collect::<Vec<_>>(), &[10]);
        assert_eq!(&ChunkedRange::new(100..101, 32).collect::<Vec<_>>(), &[1]);
        assert_eq!(&ChunkedRange::new(0..100, 32).collect::<Vec<_>>(), &[32, 32, 32, 4]);
        assert_eq!(&ChunkedRange::new(10..100, 32).collect::<Vec<_>>(), &[32, 32, 26]);
        assert_eq!(
            &ChunkedRange::new((u32::MAX as u64)..(u32::MAX as u64 + 80), 32).collect::<Vec<_>>(),
            &[32, 32, 16]
        );
        assert_eq!(
            &ChunkedRange::new((u64::MAX - 50)..u64::MAX, 32).collect::<Vec<_>>(),
            &[32, 18]
        );
    }

    #[test]
    fn test_build_with_buffer() {
        let mut builder = SparseImageBuilder::new();
        builder.max_chunk_size = BLK_SIZE;
        let mut buf = Vec::with_capacity((BLK_SIZE * 2) as usize);
        let part1 = vec![0xABu8; BLK_SIZE as usize];
        let part2 = vec![0xCDu8; BLK_SIZE as usize];
        buf.extend_from_slice(&part1);
        buf.extend_from_slice(&part2);
        let mut output = vec![];
        let builder = builder.add_source(DataSource::Buffer(buf.into_boxed_slice()));
        let expected_size = builder.built_size() as usize;
        builder.build(&mut Cursor::new(&mut output)).unwrap();
        assert_eq!(output.len(), expected_size);

        let reader = SparseReader::new(Cursor::new(&output)).unwrap();
        assert_eq!(
            reader.chunks(),
            &[
                (
                    Chunk::Raw { start: 0, size: BLK_SIZE.into() },
                    Some((SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE) as u64)
                ),
                (
                    Chunk::Raw { start: BLK_SIZE as u64, size: BLK_SIZE.into() },
                    Some((SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 2 + BLK_SIZE) as u64)
                )
            ]
        );
        assert_eq!(
            &output[(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE) as usize
                ..(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE + BLK_SIZE) as usize],
            &part1
        );
        assert_eq!(
            &output[(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 2 + BLK_SIZE) as usize
                ..(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 2 + BLK_SIZE * 2) as usize],
            &part2
        );
    }

    #[test]
    fn test_build_with_reader() {
        let part1 = vec![0xABu8; BLK_SIZE as usize];
        let part2 = vec![0xCDu8; BLK_SIZE as usize];
        let mut buf = Vec::with_capacity(BLK_SIZE as usize * 2);
        buf.extend_from_slice(&part1);
        buf.extend_from_slice(&part2);

        let mut builder = SparseImageBuilder::new();
        builder.max_chunk_size = BLK_SIZE;
        let mut output = vec![];

        let reader1 = Cursor::new(buf.clone());
        let mut reader2 = Cursor::new(buf);
        reader2.seek(SeekFrom::Start(BLK_SIZE as u64)).unwrap();

        let builder = builder
            .add_source(DataSource::Reader {
                reader: Box::new(reader1),
                size: (BLK_SIZE * 2) as u64,
            })
            .add_source(DataSource::Reader { reader: Box::new(reader2), size: BLK_SIZE as u64 });
        let expected_size = builder.built_size() as usize;
        builder.build(&mut Cursor::new(&mut output)).unwrap();
        assert_eq!(output.len(), expected_size);

        let reader = SparseReader::new(Cursor::new(&output)).unwrap();
        assert_eq!(
            reader.chunks(),
            &[
                (
                    Chunk::Raw { start: 0, size: BLK_SIZE.into() },
                    Some((SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE) as u64)
                ),
                (
                    Chunk::Raw { start: BLK_SIZE as u64, size: BLK_SIZE.into() },
                    Some((SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 2 + BLK_SIZE) as u64)
                ),
                (
                    Chunk::Raw { start: (BLK_SIZE * 2) as u64, size: BLK_SIZE.into() },
                    Some((SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 3 + BLK_SIZE * 2) as u64)
                ),
            ]
        );
        assert_eq!(
            &output[(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE) as usize
                ..(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE + BLK_SIZE) as usize],
            &part1
        );
        assert_eq!(
            &output[(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 2 + BLK_SIZE) as usize
                ..(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 2 + BLK_SIZE * 2) as usize],
            &part2
        );
        assert_eq!(
            &output[(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 3 + BLK_SIZE * 2) as usize
                ..(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 3 + BLK_SIZE * 3) as usize],
            &part2
        );
    }

    #[test]
    fn test_build_with_skip() {
        let mut builder = SparseImageBuilder::new();
        builder.max_chunk_size = BLK_SIZE;
        let mut output = vec![];
        let builder = builder.add_source(DataSource::Skip((BLK_SIZE * 2) as u64));
        let expected_size = builder.built_size() as usize;
        builder.build(&mut Cursor::new(&mut output)).unwrap();
        assert_eq!(output.len(), expected_size);

        let reader = SparseReader::new(Cursor::new(&output)).unwrap();
        assert_eq!(
            reader.chunks(),
            &[
                (Chunk::DontCare { start: 0, size: BLK_SIZE.into() }, None),
                (Chunk::DontCare { start: BLK_SIZE as u64, size: BLK_SIZE.into() }, None)
            ]
        );
    }

    #[test]
    fn test_build_with_fill() {
        let mut builder = SparseImageBuilder::new();
        builder.max_chunk_size = BLK_SIZE;
        let mut output = vec![];
        let builder = builder.add_source(DataSource::Fill(0xAB, (BLK_SIZE / 2) as u64));
        let expected_size = builder.built_size() as usize;
        builder.build(&mut Cursor::new(&mut output)).unwrap();
        assert_eq!(output.len(), expected_size);

        let reader = SparseReader::new(Cursor::new(&output)).unwrap();
        assert_eq!(
            reader.chunks(),
            &[
                (Chunk::Fill { start: 0, size: BLK_SIZE.into(), value: 0xAB }, None),
                (Chunk::Fill { start: BLK_SIZE as u64, size: BLK_SIZE.into(), value: 0xAB }, None)
            ]
        );
    }

    #[test]
    fn test_overflow_block_count() {
        struct Sink;

        impl Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl Seek for Sink {
            fn seek(&mut self, _pos: SeekFrom) -> std::io::Result<u64> {
                Ok(0)
            }
        }

        let result = SparseImageBuilder::new()
            .set_block_size(16)
            .add_source(DataSource::Skip(u64::MAX - 15))
            .build(&mut Sink);
        assert!(result.is_err());
    }

    #[cfg(target_os = "fuchsia")]
    #[test]
    fn test_build_with_vmo() {
        let mut builder = SparseImageBuilder::new();
        builder.max_chunk_size = BLK_SIZE;
        let size = (BLK_SIZE * 2) as u64;
        let vmo = zx::Vmo::create(size).unwrap();
        const PART_1: [u8; BLK_SIZE as usize] = [0xABu8; BLK_SIZE as usize];
        const PART_2: [u8; BLK_SIZE as usize] = [0xCBu8; BLK_SIZE as usize];
        vmo.write(&PART_1, 0).unwrap();
        vmo.write(&PART_2, BLK_SIZE as u64).unwrap();
        // We add two separate data sources sharing the same VMO but with a different offset.
        let mut output = vec![];
        let builder = builder
            .add_source(DataSource::Vmo {
                vmo: vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                size: BLK_SIZE as u64,
                offset: 0,
            })
            .add_source(DataSource::Vmo { vmo, size: BLK_SIZE as u64, offset: BLK_SIZE as u64 });
        let expected_size = builder.built_size() as usize;
        builder.build(&mut Cursor::new(&mut output)).unwrap();
        assert_eq!(output.len(), expected_size);

        let reader = SparseReader::new(Cursor::new(&output)).unwrap();
        assert_eq!(
            reader.chunks(),
            &[
                (
                    Chunk::Raw { start: 0, size: BLK_SIZE.into() },
                    Some((SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE) as u64)
                ),
                (
                    Chunk::Raw { start: BLK_SIZE as u64, size: BLK_SIZE.into() },
                    Some((SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 2 + BLK_SIZE) as u64)
                )
            ]
        );
        assert_eq!(
            output[(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE) as usize
                ..(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE + BLK_SIZE) as usize],
            PART_1
        );
        assert_eq!(
            output[(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 2 + BLK_SIZE) as usize
                ..(SPARSE_HEADER_SIZE + CHUNK_HEADER_SIZE * 2 + BLK_SIZE * 2) as usize],
            PART_2
        );
    }
}

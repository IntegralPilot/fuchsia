// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::checksum::{Checksum, fletcher64};
use crate::object_store::journal::{BLOCK_SIZE, JournalCheckpoint, JournalHandle, RESET_XOR};
use crate::serialized_types::{Version, Versioned, VersionedLatest};
use anyhow::{Context, Error, bail};
use byteorder::{ByteOrder, LittleEndian};
use storage_units::BlockSize;

/// JournalReader supports reading from a journal file which consist of blocks that have a trailing
/// fletcher64 checksum in the last 8 bytes of each block.  The checksum takes the check-sum of the
/// preceding block as an input to the next block so that merely copying a block to a different
/// location will cause a checksum failure.  The serialization of a single record *must* fit within
/// a single block.
pub struct JournalReader {
    // The handle of the journal file that we are reading.
    handle: Box<dyn JournalHandle>,

    // The currently buffered data.
    buf: Vec<u8>,

    // The range within the buffer containing outstanding data.
    buf_range: std::ops::Range<usize>,

    // The next offset we should read from in handle.
    read_offset: u64,

    // The file offset for next record due to be deserialized.
    buf_file_offset: u64,

    // The checksums required for the currently buffered data.  The first checksum is for
    // the *preceding* block since that is what is required to generate a checkpoint.
    checksums: Vec<Checksum>,

    // The version of types to deserialize.
    version: Version,

    // Indicates a bad checksum has been detected and no more data can be read.
    bad_checksum: bool,

    // Indicates a reset checksum has been detected.
    found_reset: bool,

    // Indicates whether the next read is the first read.
    first_read: bool,

    /// For the filesystem journal, we should never hit the end of the file but
    /// when initially reading the superblock, we only know the first block.
    /// In this case, we don't treat EOF in fill_buf as an error.
    eof_ok: bool,
}

impl JournalReader {
    pub(super) fn new(handle: impl JournalHandle, checkpoint: &JournalCheckpoint) -> Self {
        JournalReader {
            handle: Box::new(handle),
            buf: Vec::new(),
            buf_range: 0..0,
            read_offset: BLOCK_SIZE.align_down(checkpoint.file_offset),
            buf_file_offset: checkpoint.file_offset,
            checksums: vec![checkpoint.checksum],
            version: checkpoint.version,
            bad_checksum: false,
            found_reset: false,
            first_read: true,
            eof_ok: false,
        }
    }

    pub(super) fn journal_file_checkpoint(&self) -> JournalCheckpoint {
        JournalCheckpoint {
            file_offset: self.buf_file_offset,
            checksum: self.checksums[0],
            version: self.version,
        }
    }

    fn last_read_checksum(&self) -> Checksum {
        *self.checksums.last().unwrap()
    }

    /// Sets the version of Key and Value to deserialize.
    /// This can change across Journal RESET boundaries when the tail of the journal is written to
    /// by newer versions of the code.
    pub fn set_version(&mut self, version: Version) {
        self.version = version;

        // Reset bad_checksum in case this version change triggers a change in the block size.
        self.bad_checksum = false;
    }

    /// Used by superblock reader where we expect to hit EOF until we hit the first extent record.
    pub fn set_eof_ok(&mut self) {
        self.eof_ok = true;
    }

    /// To allow users to flush a block, they can store a record that indicates the rest of the
    /// block should be skipped.  When that record is read, users should call this function.
    pub fn skip_to_end_of_block(&mut self) {
        let bs = self.block_size();
        let block_offset = self.buf_file_offset % bs;
        if block_offset > 0 {
            self.consume((bs - block_offset) as usize - std::mem::size_of::<Checksum>());
        }
    }

    pub fn handle(&mut self) -> &mut dyn JournalHandle {
        self.handle.as_mut()
    }

    /// Tries to deserialize a record of type T from the journal stream.  It might return
    /// ReadResult::Reset if a reset marker is encountered, or ReadResult::ChecksumMismatch (which
    /// is expected at the end of the journal stream).
    pub async fn deserialize<T: VersionedLatest>(&mut self) -> Result<ReadResult<T>, Error>
    where
        T: Versioned,
    {
        self.fill_buf().await?;
        let mut cursor = std::io::Cursor::new(self.buffer());
        let version = self.version;
        match T::deserialize_from_version(&mut cursor, version) {
            Ok(record) => {
                let consumed = cursor.position() as usize;
                self.consume(consumed);
                Ok(ReadResult::Some(record))
            }
            Err(e) => {
                if self.found_reset {
                    self.found_reset = false;

                    // Fix up buf_range now...
                    self.consume(self.buf_range.end - self.buf_range.start);
                    self.buf_range =
                        self.buf_range.end..self.buf.len() - std::mem::size_of::<Checksum>();
                    let (version, to_consume) = {
                        let mut cursor = std::io::Cursor::new(self.buffer());
                        let version = Version::deserialize_from(&mut cursor)
                            .context("Failed to deserialize version")?;
                        (version, cursor.position() as usize)
                    };
                    self.version = version.clone();
                    self.consume(to_consume);
                    return Ok(ReadResult::Reset(version));
                } else if let Some(io_error) = e.downcast_ref::<std::io::Error>() {
                    if io_error.kind() == std::io::ErrorKind::UnexpectedEof && self.bad_checksum {
                        return Ok(ReadResult::ChecksumMismatch);
                    }
                }
                Err(e.into())
            }
        }
    }

    // Fills the buffer so that at least one blocks worth of data is contained within the buffer.
    // After reading a block, it verifies the checksum.  Once done, it should be possible to
    // deserialize records.
    pub async fn fill_buf(&mut self) -> Result<(), Error> {
        let block_size = self.block_size();
        let bs = block_size.get() as usize;
        let min_required = bs - std::mem::size_of::<Checksum>();

        if self.found_reset
            || self.bad_checksum
            || self.buf_range.end - self.buf_range.start >= min_required
        {
            return Ok(());
        }

        // Before we read, shuffle existing data to the beginning of the buffer.
        self.buf.copy_within(self.buf_range.clone(), 0);
        self.buf_range = 0..self.buf_range.end - self.buf_range.start;

        while self.buf_range.end - self.buf_range.start < min_required {
            self.buf.resize(self.buf_range.end + bs, 0);
            let last_read_checksum = self.last_read_checksum();

            // Read the next block's worth, verify its checksum, and append it to |buf|.
            let mut buffer = self.handle.allocate_buffer(bs).await;
            assert!(block_size.is_aligned(self.read_offset));
            let bytes_read = self.handle.read(self.read_offset, buffer.as_mut()).await?;
            if bytes_read != bs {
                if self.eof_ok {
                    return Ok(());
                } else {
                    bail!("unexpected end of journal file");
                }
            }
            buffer.copy_to_slice(&mut self.buf.as_mut_slice()[self.buf_range.end..]);

            let (contents_slice, checksum_slice) = self.buf.as_slice()
                [self.buf_range.end..self.buf_range.end + bs]
                .split_at(bs - std::mem::size_of::<Checksum>());
            let stored_checksum = LittleEndian::read_u64(checksum_slice);

            match verify_checksum(
                contents_slice,
                stored_checksum,
                last_read_checksum,
                !self.first_read,
            ) {
                ChecksumResult::Ok => {}
                ChecksumResult::Reset => {
                    // Record that we've encountered a reset in the stream (a point where the
                    // journal wasn't cleanly closed in the past) and it starts afresh in this
                    // block.  We don't adjust buf_range until the reset has been processed, but
                    // we do push the checksum.
                    self.found_reset = true;
                    // We need to adjust the last pushed checksum since it could possibly be
                    // for checkpoints later (see Self::journal_file_checkpoint), and
                    // checkpoints should always use the correct seed.
                    if let Some(checksum) = self.checksums.last_mut() {
                        *checksum ^= RESET_XOR;
                    }
                    self.checksums.push(stored_checksum);
                    self.read_offset += block_size;
                    return Ok(());
                }
                ChecksumResult::Bad => {
                    self.bad_checksum = true;
                    return Ok(());
                }
            }

            self.first_read = false;
            self.checksums.push(stored_checksum);

            // If this was our first read, our buffer offset might be somewhere within the middle of
            // the block, so we need to adjust buf_range accordingly.
            if self.buf_file_offset > self.read_offset {
                assert!(self.buf_range.start == 0);
                self.buf_range = (self.buf_file_offset - self.read_offset) as usize
                    ..self.buf.len() - std::mem::size_of::<Checksum>();
            } else {
                self.buf_range =
                    self.buf_range.start..self.buf.len() - std::mem::size_of::<Checksum>();
            }
            self.read_offset += block_size;
        }
        Ok(())
    }

    // Used after deserializing to indicate how many bytes were deserialized and adjusts the buffer
    // pointers accordingly.
    pub fn consume(&mut self, amount: usize) {
        let bs = self.block_size();
        assert!(amount < bs.get() as usize);
        let block_offset_before = self.buf_file_offset % bs;
        self.buf_file_offset += amount as u64;
        // If we crossed a block boundary, then the file offset needs be incremented by the size of
        // the checksum.
        if block_offset_before + amount as u64 >= bs - std::mem::size_of::<Checksum>() as u64 {
            self.buf_file_offset += std::mem::size_of::<Checksum>() as u64;
            self.checksums.drain(0..1);
        }
        self.buf_range.start += amount
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buf[self.buf_range.clone()]
    }

    fn block_size(&self) -> BlockSize {
        BLOCK_SIZE
    }
}

enum ChecksumResult {
    Ok,
    Reset,
    Bad,
}

fn verify_checksum(
    contents: &[u8],
    stored_checksum: u64,
    last_read_checksum: u64,
    allow_reset: bool,
) -> ChecksumResult {
    if stored_checksum == fletcher64(contents, last_read_checksum) {
        ChecksumResult::Ok
    } else {
        if allow_reset {
            if stored_checksum == fletcher64(contents, last_read_checksum ^ RESET_XOR) {
                return ChecksumResult::Reset;
            }
        }
        ChecksumResult::Bad
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum ReadResult<T> {
    Reset(Version),
    Some(T),
    ChecksumMismatch,
}

#[cfg(test)]
mod tests {
    // The following tests use JournalWriter to test our reader implementation. This works so long
    // as JournalWriter doesn't use JournalReader to test its implementation.
    use super::{BLOCK_SIZE, JournalReader, ReadResult};
    use crate::object_handle::{ObjectHandle, WriteObjectHandle};
    use crate::object_store::journal::writer::JournalWriter;
    use crate::object_store::journal::{Checksum, JournalCheckpoint, RESET_XOR};
    use crate::serialized_types::{LATEST_VERSION, Version, VersionedLatest};
    use crate::testing::fake_object::{FakeObject, FakeObjectHandle};
    use std::io::Write;
    use std::sync::Arc;

    async fn write_items<T: VersionedLatest + std::fmt::Debug>(
        handle: FakeObjectHandle,
        items: &[T],
    ) {
        let mut writer = JournalWriter::new(BLOCK_SIZE, 0);
        for item in items {
            writer.write_record(item).unwrap();
        }
        writer.pad_to_block().expect("pad_to_block failed");
        let mut buf = handle.allocate_buffer(writer.flushable_bytes()).await;
        let offset = writer.take_flushable(buf.as_mut());
        handle.write_or_append(Some(offset), buf.as_ref()).await.expect("overwrite failed");
    }

    #[fuchsia::test]
    async fn test_read_single_record() {
        let object = Arc::new(FakeObject::new());
        let handle = FakeObjectHandle::new(object.clone());
        // Make the journal file a minimum of two blocks since reading to EOF is an error.
        let len = (BLOCK_SIZE * 2) as usize;
        let mut buf = handle.allocate_buffer(len).await;
        buf.fill(0u8);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        write_items(FakeObjectHandle::new(object.clone()), &[4u32]).await;

        let mut reader = JournalReader::new(
            FakeObjectHandle::new(object.clone()),
            &JournalCheckpoint { version: LATEST_VERSION, ..JournalCheckpoint::default() },
        );
        let value = reader.deserialize().await.expect("deserialize failed");
        assert_eq!(value, ReadResult::Some(4u32));
    }

    #[fuchsia::test]
    async fn test_journal_file_checkpoint() {
        let object = Arc::new(FakeObject::new());
        let checkpoint =
            JournalCheckpoint { version: LATEST_VERSION, ..JournalCheckpoint::default() };
        let mut reader = JournalReader::new(FakeObjectHandle::new(object.clone()), &checkpoint);
        assert_eq!(reader.journal_file_checkpoint(), checkpoint);
        // Make the journal file a minimum of two blocks since reading to EOF is an error.
        let handle = FakeObjectHandle::new(object.clone());
        let len = (BLOCK_SIZE * 2) as usize;
        let mut buf = handle.allocate_buffer(len).await;
        buf.fill(0u8);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        write_items(FakeObjectHandle::new(object.clone()), &[4u32, 7u32]).await;

        assert_eq!(reader.deserialize().await.expect("deserialize failed"), ReadResult::Some(4u32));

        // If we take the checkpoint here and then create another reader, we should see the second
        // item.
        let checkpoint = reader.journal_file_checkpoint();
        let mut reader = JournalReader::new(FakeObjectHandle::new(object.clone()), &checkpoint);
        assert_eq!(reader.deserialize().await.expect("deserialize failed"), ReadResult::Some(7u32));
    }

    #[fuchsia::test]
    async fn test_skip_to_end_of_block() {
        let object = Arc::new(FakeObject::new());
        // Make the journal file a minimum of two blocks since reading to EOF is an error.
        let handle = FakeObjectHandle::new(object.clone());
        let len = (BLOCK_SIZE * 3) as usize;
        let mut buf = handle.allocate_buffer(len).await;
        buf.fill(0u8);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        let mut writer = JournalWriter::new(BLOCK_SIZE, 0);
        writer.write_record(&4u32).unwrap();
        writer.pad_to_block().expect("pad_to_block failed");
        writer.write_record(&7u32).unwrap();
        writer.pad_to_block().expect("pad_to_block failed");
        let mut buf = handle.allocate_buffer(writer.flushable_bytes()).await;
        let offset = writer.take_flushable(buf.as_mut());
        handle.write_or_append(Some(offset), buf.as_ref()).await.expect("overwrite failed");
        let mut reader = JournalReader::new(
            FakeObjectHandle::new(object.clone()),
            &JournalCheckpoint { version: LATEST_VERSION, ..JournalCheckpoint::default() },
        );
        assert_eq!(reader.deserialize().await.expect("deserialize failed"), ReadResult::Some(4u32));
        reader.skip_to_end_of_block();
        assert_eq!(reader.deserialize().await.expect("deserialize failed"), ReadResult::Some(7u32));
    }

    #[fuchsia::test]
    async fn test_handle() {
        let object = Arc::new(FakeObject::new());
        // Make the journal file a minimum of two blocks since reading to EOF is an error.
        let handle = FakeObjectHandle::new(object.clone());
        let len = (BLOCK_SIZE * 3) as usize;
        let mut buf = handle.allocate_buffer(len).await;
        buf.fill(0u8);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        let mut reader = JournalReader::new(
            FakeObjectHandle::new(object.clone()),
            &JournalCheckpoint { version: LATEST_VERSION, ..JournalCheckpoint::default() },
        );
        assert_eq!(reader.handle().get_size(), BLOCK_SIZE * 3);
    }

    #[fuchsia::test]
    async fn test_item_spanning_block() {
        let object = Arc::new(FakeObject::new());
        // Make the journal file a minimum of two blocks since reading to EOF is an error.
        let handle = FakeObjectHandle::new(object.clone());
        let len = (BLOCK_SIZE * 3) as usize;
        let mut buf = handle.allocate_buffer(len).await;
        buf.fill(0u8);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        let mut writer = JournalWriter::new(BLOCK_SIZE, 0);
        // Write one byte so that everything else is misaligned.
        writer.write_record(&4u8).unwrap();
        let mut count: i32 = 0;
        while writer.journal_file_checkpoint().file_offset < BLOCK_SIZE {
            writer.write_record(&12345678u32).unwrap();
            count += 1;
        }
        // Check that writing didn't end up being aligned on a block.
        assert_ne!(writer.journal_file_checkpoint().file_offset, BLOCK_SIZE);
        writer.pad_to_block().expect("pad_to_block failed");
        let mut buf = handle.allocate_buffer(writer.flushable_bytes()).await;
        let offset = writer.take_flushable(buf.as_mut());
        handle.write_or_append(Some(offset), buf.as_ref()).await.expect("overwrite failed");

        let mut reader = JournalReader::new(
            FakeObjectHandle::new(object.clone()),
            &JournalCheckpoint { version: LATEST_VERSION, ..JournalCheckpoint::default() },
        );
        assert_eq!(reader.deserialize().await.expect("deserialize failed"), ReadResult::Some(4u8));
        for _ in 0..count {
            assert_eq!(
                reader.deserialize().await.expect("deserialize failed"),
                ReadResult::Some(12345678u32)
            );
        }
    }

    #[fuchsia::test]
    async fn test_reset() {
        let object = Arc::new(FakeObject::new());
        // Make the journal file a minimum of two blocks since reading to EOF is an error.
        let handle = FakeObjectHandle::new(object.clone());
        let len = (BLOCK_SIZE * 3) as usize;
        let mut buf = handle.allocate_buffer(len).await;
        buf.fill(0u8);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        write_items(FakeObjectHandle::new(object.clone()), &[4u32, 7u32]).await;

        let mut reader = JournalReader::new(
            FakeObjectHandle::new(object.clone()),
            &JournalCheckpoint { version: LATEST_VERSION, ..JournalCheckpoint::default() },
        );
        assert_eq!(reader.deserialize().await.expect("deserialize failed"), ReadResult::Some(4u32));
        assert_eq!(reader.deserialize().await.expect("deserialize failed"), ReadResult::Some(7u32));
        reader.skip_to_end_of_block();
        assert_eq!(
            reader.deserialize::<u32>().await.expect("deserialize failed"),
            ReadResult::ChecksumMismatch
        );

        let mut writer = JournalWriter::new(BLOCK_SIZE, 0);
        let new_version = Version { minor: LATEST_VERSION.minor + 1, ..LATEST_VERSION };
        writer.seek(JournalCheckpoint {
            file_offset: BLOCK_SIZE.get(),
            checksum: reader.last_read_checksum() ^ RESET_XOR,
            version: new_version,
        });
        new_version.serialize_into(&mut writer).expect("write version failed");
        writer.write_record(&13u32).unwrap();
        let checkpoint = writer.journal_file_checkpoint();
        writer.write_record(&78u32).unwrap();
        writer.pad_to_block().expect("pad_to_block failed");
        writer.write_record(&90u32).unwrap();
        writer.pad_to_block().expect("pad_to_block failed");
        let mut buf = handle.allocate_buffer(writer.flushable_bytes()).await;
        let offset = writer.take_flushable(buf.as_mut());
        handle.write_or_append(Some(offset), buf.as_ref()).await.expect("overwrite failed");

        let mut reader = JournalReader::new(
            FakeObjectHandle::new(object.clone()),
            &JournalCheckpoint { version: new_version, ..JournalCheckpoint::default() },
        );
        assert_eq!(reader.deserialize().await.expect("deserialize failed"), ReadResult::Some(4u32));
        assert_eq!(reader.deserialize().await.expect("deserialize failed"), ReadResult::Some(7u32));
        reader.skip_to_end_of_block();
        assert_eq!(
            reader.deserialize::<u32>().await.expect("deserialize failed"),
            ReadResult::Reset(new_version),
        );
        assert_eq!(
            reader.deserialize().await.expect("deserialize failed"),
            ReadResult::Some(13u32)
        );
        assert_eq!(reader.journal_file_checkpoint(), checkpoint);
        assert_eq!(
            reader.deserialize().await.expect("deserialize failed"),
            ReadResult::Some(78u32)
        );
        reader.skip_to_end_of_block();
        assert_eq!(
            reader.deserialize().await.expect("deserialize failed"),
            ReadResult::Some(90u32)
        );

        // Make sure a reader can start from the middle of a reset block.
        let mut reader = JournalReader::new(FakeObjectHandle::new(object.clone()), &checkpoint);
        assert_eq!(
            reader.deserialize().await.expect("deserialize failed"),
            ReadResult::Some(78u32)
        );
        reader.skip_to_end_of_block();
        assert_eq!(
            reader.deserialize().await.expect("deserialize failed"),
            ReadResult::Some(90u32)
        );
    }

    #[fuchsia::test]
    async fn test_reader_starting_near_end_of_block() {
        let object = Arc::new(FakeObject::new());
        // Make the journal file a minimum of two blocks since reading to EOF is an error.
        let handle = FakeObjectHandle::new(object.clone());
        let len = (BLOCK_SIZE * 3) as usize;
        let mut buf = handle.allocate_buffer(len).await;
        buf.fill(0u8);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        let mut writer = JournalWriter::new(BLOCK_SIZE, 0);
        let len = 2 * (BLOCK_SIZE.get() as usize - std::mem::size_of::<Checksum>());
        assert_eq!(writer.write(&vec![78u8; len]).expect("write failed"), len);
        let mut buf = handle.allocate_buffer(writer.flushable_bytes()).await;
        let offset = writer.take_flushable(buf.as_mut());
        handle.write_or_append(Some(offset), buf.as_ref()).await.expect("overwrite failed");

        let checkpoint = JournalCheckpoint {
            file_offset: BLOCK_SIZE - std::mem::size_of::<Checksum>() as u64 - 1,
            checksum: 0,
            version: LATEST_VERSION,
        };
        let mut reader = JournalReader::new(FakeObjectHandle::new(object.clone()), &checkpoint);
        let mut offset = checkpoint.file_offset as usize;
        while offset < len {
            reader.fill_buf().await.expect("fill_buf failed");
            let mut buf = reader.buffer();
            let amount = std::cmp::min(buf.len(), len - offset);
            buf = &buf[..amount];
            assert_eq!(&buf[..amount], vec![78; amount]);
            offset += amount;
            reader.consume(amount);
        }
    }

    #[fuchsia::test]
    async fn test_write_to_block_boundary() {
        let object = Arc::new(FakeObject::new());
        // Make the journal file a minimum of two blocks since reading to EOF is an error.
        let handle = FakeObjectHandle::new(object.clone());
        let len = (BLOCK_SIZE * 3) as usize;
        let mut buf = handle.allocate_buffer(len).await;
        buf.fill(0u8);
        handle.write_or_append(Some(0), buf.as_ref()).await.expect("write failed");
        let mut writer = JournalWriter::new(BLOCK_SIZE, 0);
        let len = BLOCK_SIZE.get() as usize - std::mem::size_of::<Checksum>();
        assert_eq!(writer.write(&vec![78u8; len]).expect("write failed"), len);
        let mut buf = handle.allocate_buffer(writer.flushable_bytes()).await;
        let offset = writer.take_flushable(buf.as_mut());
        handle.write_or_append(Some(offset), buf.as_ref()).await.expect("overwrite failed");
        assert_eq!(offset, 0);

        // The checkpoint should be at the start of the next block.
        let writer_checkpoint = writer.journal_file_checkpoint();
        assert_eq!(writer_checkpoint.file_offset, BLOCK_SIZE);

        let mut reader = JournalReader::new(
            FakeObjectHandle::new(object.clone()),
            &JournalCheckpoint::default(),
        );
        reader.set_version(LATEST_VERSION);

        reader.fill_buf().await.expect("fill_buf failed");

        assert_eq!(&reader.buffer()[..len], &buf.subslice(..len).to_vec());

        reader.consume(len);

        assert_eq!(reader.journal_file_checkpoint(), writer_checkpoint);
    }
}

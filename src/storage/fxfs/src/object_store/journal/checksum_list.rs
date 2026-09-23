// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::checksum::{Checksum, fletcher64_ptr};
use crate::errors::FxfsError;
use crate::range::RangeExt;
use anyhow::{Error, anyhow, ensure};
use std::collections::BTreeMap;
use std::ops::Range;
use storage_device::Device;

#[derive(Clone, Debug, Eq, PartialEq)]
enum ChecksumState {
    Unverified(Vec<Checksum>),
    Valid,
    Invalid,
}

impl ChecksumState {
    fn checksum(&mut self) -> Option<&mut Vec<Checksum>> {
        if let ChecksumState::Unverified(c) = self { Some(c) } else { None }
    }
}

#[derive(Clone, Debug)]
struct ChecksumEntry {
    // |start| is the journal_offset at which this range was written.
    start_journal_offset: u64,
    device_range: Range<u64>,
    // Holds checksums that cover |device_range| that should hold valid from
    // start_journal_offset..end_journal_offset.
    // |end_journal_offset| is the journal_offset at which the checksum range was deallocated.
    // |end_journal_offset| defaults to u64::MAX but may be lowered if a range is deallocated.
    checksums: Vec<(ChecksumState, /* end_journal_offset */ u64)>,
}

#[derive(Clone, Default)]
pub struct ChecksumList {
    // The offset that is known to have been flushed to the device.  Any entries in the journal that
    // are prior to this point are ignored since there is no need to verify those checksums.
    flushed_offset: u64,

    // This is a list of checksums that we might need to verify, in journal order.
    checksum_entries: Vec<ChecksumEntry>,

    // Records a mapping from the *ending* offset of the device range, to the entry index.
    device_offset_to_checksum_entry: BTreeMap<u64, usize>,

    // The maximum chunk size within checksum_entries which determines the size of the buffer we
    // need to allocate during verification.
    max_chunk_size: u64,
}

impl ChecksumList {
    pub fn new(flushed_offset: u64) -> Self {
        ChecksumList { flushed_offset, ..Default::default() }
    }

    /// Adds an extent that might need its checksum verifying.  Extents must be pushed in
    /// journal-offset order.
    pub fn push(
        &mut self,
        journal_offset: u64,
        mut device_range: Range<u64>,
        mut checksums: &[u64],
        first_write: bool,
    ) -> Result<(), Error> {
        if journal_offset < self.flushed_offset {
            // Ignore anything that was prior to being flushed.
            return Ok(());
        }
        let chunk_size = device_range.length().unwrap() / checksums.len() as u64;
        if chunk_size > self.max_chunk_size {
            self.max_chunk_size = chunk_size;
        }

        // Copy on write extents will always write to new device ranges. However, overwrite extents
        // will continue to write to the same device range repeatedly. On every write, a new
        // checksum is emitted for each block in the range.
        //
        // When replaying the journal, it's valid for any of the possible checksums to match,
        // because any or all of the future writes to the range may have gone through. The only
        // time that matters is when the first write to that block has happened during this replay,
        // since that's the only thing we can actually roll back. On the first write, we create the
        // checksum entry, and if any further checksums come in, we add them to the existing list.
        //
        // Fxfs currently doesn't support cloning, but if and when it does, this code may need to
        // be revisited.
        let mut gap_entries = Vec::new();
        let mut r = self.device_offset_to_checksum_entry.range(device_range.start + 1..);
        while let Some((_, index)) = r.next() {
            let entry = &mut self.checksum_entries[*index];
            if entry.device_range.start >= device_range.end {
                break;
            }

            // Put the gap aside for later.
            let gap = device_range.start..entry.device_range.start;
            let gap_checksums = if gap.is_empty() {
                &[]
            } else {
                let (head, tail) =
                    checksums.split_at(((gap.end - gap.start) / chunk_size) as usize);
                checksums = tail;
                head
            };

            // Check that the checksums match.
            ensure!(
                (entry.device_range.end - entry.device_range.start) / entry.checksums.len() as u64
                    == chunk_size,
                anyhow!(FxfsError::Inconsistent)
                    .context("Wrong number of checksums for device range"),
            );

            let overlap = std::cmp::max(device_range.start, entry.device_range.start)
                ..std::cmp::min(device_range.end, entry.device_range.end);
            let entry_checksums = &mut entry.checksums[((overlap.start - entry.device_range.start)
                / chunk_size) as usize
                ..((overlap.end - entry.device_range.start) / chunk_size) as usize];

            let (head, tail) = checksums.split_at(entry_checksums.len());
            checksums = tail;
            for ((checksum_state, _), new_checksum) in entry_checksums.iter_mut().zip(head.iter()) {
                checksum_state.checksum().unwrap().push(*new_checksum);
            }
            device_range.start = overlap.end;

            // Now that we no longer need entry, we can insert the gap into checksum_entries, but we
            // can't touch device_offset_to_checksum_entry until after the loop.
            if !gap.is_empty() && first_write {
                gap_entries.push((gap.end, self.checksum_entries.len()));
                self.checksum_entries.push(ChecksumEntry {
                    start_journal_offset: journal_offset,
                    device_range: gap,
                    checksums: gap_checksums
                        .iter()
                        .map(|c| (ChecksumState::Unverified(vec![*c]), u64::MAX))
                        .collect(),
                });
            }

            if device_range.is_empty() {
                break;
            }
        }

        // Add the gap entries we couldn't add earlier.
        self.device_offset_to_checksum_entry.extend(gap_entries);

        // Add any remainder.
        if !device_range.is_empty() && first_write {
            self.device_offset_to_checksum_entry
                .insert(device_range.end, self.checksum_entries.len());
            self.checksum_entries.push(ChecksumEntry {
                start_journal_offset: journal_offset,
                device_range,
                checksums: checksums
                    .iter()
                    .map(|c| (ChecksumState::Unverified(vec![*c]), u64::MAX))
                    .collect(),
            });
        }

        Ok(())
    }

    /// Marks an extent as deallocated.  If this journal-offset ends up being replayed, it means
    /// that we can skip a previously queued checksum.
    pub fn mark_deallocated(&mut self, journal_offset: u64, mut device_range: Range<u64>) {
        if journal_offset < self.flushed_offset {
            // Ignore anything that was prior to being flushed.
            return;
        }
        let mut r = self.device_offset_to_checksum_entry.range(device_range.start + 1..);
        while let Some((_, index)) = r.next() {
            let entry = &mut self.checksum_entries[*index];
            if entry.device_range.start >= device_range.end {
                break;
            }
            let chunk_size =
                (entry.device_range.length().unwrap() / entry.checksums.len() as u64) as usize;
            let checksum_index_start = if device_range.start < entry.device_range.start {
                0
            } else {
                (device_range.start - entry.device_range.start) as usize / chunk_size
            };
            // Figure out the overlap.
            if entry.device_range.end >= device_range.end {
                let checksum_index_end =
                    (device_range.end - entry.device_range.start) as usize / chunk_size;
                // This entry covers the remainder.
                entry.checksums[checksum_index_start..checksum_index_end]
                    .iter_mut()
                    .for_each(|c| c.1 = journal_offset);
                break;
            }
            entry.checksums[checksum_index_start..].iter_mut().for_each(|c| c.1 = journal_offset);
            device_range.start = entry.device_range.end;
        }
    }

    /// Verifies the checksums in the list.  `journal_offset` should indicate the last journal
    /// offset read and verify will return the journal offset that it is safe to replay up to.
    /// `flushed_offset` indicates the offset that we know to have been flushed and so we don't need
    /// to perform verification.
    pub async fn verify(
        &mut self,
        device: &dyn Device,
        mut journal_offset: u64,
    ) -> Result<u64, Error> {
        let mut buf = device.allocate_buffer(self.max_chunk_size as usize).await;
        'try_again: loop {
            for e in &mut self.checksum_entries {
                if e.start_journal_offset >= journal_offset {
                    break;
                }
                let chunk_size =
                    (e.device_range.length().unwrap() / e.checksums.len() as u64) as usize;
                let mut offset = e.device_range.start;
                for (checksum_state, dependency) in e.checksums.iter_mut() {
                    // We only need to verify the checksum if we know the dependency isn't going to
                    // be replayed and we can skip verifications that we know were done on the
                    // previous iteration of the loop.
                    if *dependency >= journal_offset {
                        if let Some(checksums) = checksum_state.checksum() {
                            device.read(offset, buf.subslice_mut(0..chunk_size)).await?;
                            let found_checksum =
                                fletcher64_ptr(buf.subslice(0..chunk_size).as_ptr_slice(), 0);
                            if checksums
                                .iter()
                                .find(|&&checksum| checksum == found_checksum)
                                .is_some()
                            {
                                *checksum_state = ChecksumState::Valid;
                            } else {
                                *checksum_state = ChecksumState::Invalid;
                            }
                        }
                        if *checksum_state == ChecksumState::Invalid {
                            // Verification failed, so we need to reset the journal_offset to
                            // before this entry and try again.
                            journal_offset = e.start_journal_offset;
                            continue 'try_again;
                        }
                    }
                    offset += chunk_size as u64;
                }
            }
            return Ok(journal_offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChecksumList;
    use crate::checksum::fletcher64;
    use storage_device::Device;
    use storage_device::fake_device::FakeDevice;

    #[fuchsia::test]
    async fn test_verify() {
        let device = FakeDevice::new(2048, 512);
        let mut buffer = device.allocate_buffer(2048).await;
        let mut list = ChecksumList::new(0);

        buffer.as_mut().subslice_mut(0..512).copy_from_slice(&[1; 512]);
        buffer.as_mut().subslice_mut(512..1024).copy_from_slice(&[2; 512]);
        buffer.as_mut().subslice_mut(1024..1536).copy_from_slice(&[3; 512]);
        buffer.as_mut().subslice_mut(1536..2048).copy_from_slice(&[4; 512]);
        device.write(512, buffer.as_ref()).await.expect("write failed");
        list.push(
            1,
            512..2048,
            &[fletcher64(&[1; 512], 0), fletcher64(&[2; 512], 0), fletcher64(&[3; 512], 0)],
            true,
        )
        .unwrap();

        // All entries should pass.
        assert_eq!(list.clone().verify(&device, 10).await.expect("verify failed"), 10);

        // Corrupt the middle of the three 512 byte blocks.
        buffer.as_mut().subslice_mut(512..513).copy_from_slice(&[0]);
        device.write(512, buffer.as_ref()).await.expect("write failed");

        // Verification should fail now.
        assert_eq!(list.clone().verify(&device, 10).await.expect("verify failed"), 1);

        // Mark the middle block as deallocated and then it should pass again.
        list.mark_deallocated(2, 1024..1536);
        assert_eq!(list.clone().verify(&device, 10).await.expect("verify failed"), 10);

        // Add another entry followed by a deallocation.
        list.push(3, 2048..2560, &[fletcher64(&[4; 512], 0)], true).unwrap();
        list.mark_deallocated(4, 1536..2048);

        // All entries should validate.
        assert_eq!(list.clone().verify(&device, 10).await.expect("verify failed"), 10);

        // Now corrupt the block at 2048.
        buffer.as_mut().subslice_mut(1536..1537).copy_from_slice(&[0]);
        device.write(512, buffer.as_ref()).await.expect("write failed");

        // This should only validate up to journal offset 3.
        assert_eq!(list.clone().verify(&device, 10).await.expect("verify failed"), 3);

        // Corrupt the block that was marked as deallocated in #4.
        buffer.as_mut().subslice_mut(1024..1025).copy_from_slice(&[0]);
        device.write(512, buffer.as_ref()).await.expect("write failed");

        // The deallocation in #4 should be ignored and so validation should only succeed up
        // to offset 1.
        assert_eq!(list.verify(&device, 10).await.expect("verify failed"), 1);
    }

    #[fuchsia::test]
    async fn test_verify_entry_prior_to_flushed_offset_is_ignored() {
        let device = FakeDevice::new(2048, 512);
        let mut buffer = device.allocate_buffer(2048).await;
        let mut list = ChecksumList::new(2);

        buffer.as_mut().subslice_mut(0..512).copy_from_slice(&[1; 512]);
        buffer.as_mut().subslice_mut(512..1024).copy_from_slice(&[2; 512]);
        device.write(512, buffer.as_ref()).await.expect("write failed");

        // This entry has the wrong checksum will fail, but it should be ignored anyway because it
        // is prior to the flushed offset.
        list.push(1, 512..1024, &[fletcher64(&[2; 512], 0)], true).unwrap();

        list.push(2, 1024..1536, &[fletcher64(&[2; 512], 0)], true).unwrap();

        assert_eq!(list.verify(&device, 10).await.expect("verify failed"), 10);
    }

    #[fuchsia::test]
    async fn test_deallocate_overlap() {
        let device = FakeDevice::new(2048, 512);
        let mut buffer = device.allocate_buffer(512).await;
        let mut list = ChecksumList::new(1);

        buffer.copy_from_slice(&[2; 512]);
        device.write(2560, buffer.as_ref()).await.expect("write failed");

        list.push(2, 512..1024, &[fletcher64(&[1; 512], 0)], true).unwrap();
        list.mark_deallocated(3, 0..1024);
        list.push(4, 2048..3072, &[fletcher64(&[2; 512], 0); 2], true).unwrap();
        list.mark_deallocated(5, 1536..2560);

        assert_eq!(list.verify(&device, 10).await.expect("verify failed"), 10);
    }

    #[fuchsia::test]
    async fn test_different_chunk_size() {
        let device = FakeDevice::new(2048, 512);
        let mut buffer = device.allocate_buffer(1024).await;
        let mut list = ChecksumList::new(1);

        buffer.copy_from_slice(&[2; 1024]);
        device.write(1024, buffer.as_ref()).await.expect("write failed");

        let c0 = fletcher64(&[0; 512], 0);
        let c2 = fletcher64(&[2; 512], 0);

        list.push(1, 1024..2048, &[c2, c2], true).unwrap();

        list.push(2, 1024..2048, &[c0], false)
            .expect_err("Expected failure due to different chunk size");
        assert_eq!(list.verify(&device, 2).await.expect("verify failed"), 2);
    }

    #[fuchsia::test]
    async fn test_drop_unneeded_checksums_after_first_write() {
        let device = FakeDevice::new(2048, 512);
        let mut buffer = device.allocate_buffer(1024).await;
        let mut list = ChecksumList::new(1);

        buffer.copy_from_slice(&[2; 1024]);
        device.write(1024, buffer.as_ref()).await.expect("write failed");

        let c1 = fletcher64(&[1; 512], 0);
        let c2 = fletcher64(&[2; 512], 0);

        list.push(1, 1024..2048, &[c2, c2], true).unwrap();
        list.push(2, 0..1024, &[c2, c1], false).unwrap();
        assert_eq!(list.verify(&device, 2).await.expect("verify failed"), 2);
    }

    #[fuchsia::test]
    async fn test_overlapping_checksums() {
        let device = FakeDevice::new(2048, 512);
        let mut buffer = device.allocate_buffer(1024).await;
        let mut list = ChecksumList::new(1);

        buffer.copy_from_slice(&[2; 1024]);
        device.write(1024, buffer.as_ref()).await.expect("write failed");

        let c1 = fletcher64(&[1; 512], 0);
        let c2 = fletcher64(&[2; 512], 0);

        list.push(1, 1024..2048, &[c2, c2], true).unwrap();

        list.push(2, 512..2560, &[c1, c1, c1, c1], false).unwrap();
        assert_eq!(list.verify(&device, 2).await.expect("verify failed"), 2);

        // Changing the data to ones should be fine now because the other checksums will match.
        buffer.copy_from_slice(&[1; 1024]);
        device.write(1024, buffer.as_ref()).await.expect("write failed");
        assert_eq!(list.verify(&device, 4).await.expect("verify failed"), 4);
    }
}

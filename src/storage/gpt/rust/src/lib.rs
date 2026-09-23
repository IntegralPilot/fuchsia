// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, anyhow, ensure};
use block_client::{BlockClient, BufferSlice, MutableBufferSlice, RemoteBlockClient};
use fuchsia_sync::Mutex;
use std::collections::BTreeMap;
use std::sync::Arc;
use zerocopy::{FromBytes as _, IntoBytes as _};

pub mod format;

/// GPT GUIDs are stored in mixed-endian format (see Appendix A of the EFI spec).  To ensure this is
/// correctly handled, wrap the Uuid type to hide methods that use the UUIDs inappropriately.
#[derive(Clone, Default, Debug)]
pub struct Guid(uuid::Uuid);

impl From<uuid::Uuid> for Guid {
    fn from(uuid: uuid::Uuid) -> Self {
        Self(uuid)
    }
}

impl Guid {
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(uuid::Uuid::from_bytes_le(bytes))
    }

    pub fn to_bytes(&self) -> [u8; 16] {
        self.0.to_bytes_le()
    }

    pub fn to_string(&self) -> String {
        self.0.to_string()
    }

    pub fn nil() -> Self {
        Self(uuid::Uuid::nil())
    }

    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

#[derive(Clone, Debug)]
pub struct PartitionInfo {
    pub label: String,
    pub type_guid: Guid,
    pub instance_guid: Guid,
    pub start_block: u64,
    pub num_blocks: u64,
    pub flags: u64,
}

impl PartitionInfo {
    pub fn from_entry(entry: &format::PartitionTableEntry) -> Result<Self, Error> {
        let label = String::from_utf16(entry.name.split(|v| *v == 0u16).next().unwrap())?;
        ensure!(
            entry.last_lba >= entry.first_lba,
            "Partition last_lba < first_lba (first_lba: {}, last_lba: {})",
            entry.first_lba,
            entry.last_lba
        );
        let num_blocks = entry
            .last_lba
            .checked_add(1)
            .ok_or_else(|| anyhow!("Partition last_lba overflow (last_lba: {})", entry.last_lba))?
            .checked_sub(entry.first_lba)
            .ok_or_else(|| {
                anyhow!(
                    "Partition last_lba < first_lba (first_lba: {}, last_lba: {})",
                    entry.first_lba,
                    entry.last_lba
                )
            })?;
        Ok(Self {
            label,
            type_guid: Guid::from_bytes(entry.type_guid),
            instance_guid: Guid::from_bytes(entry.instance_guid),
            start_block: entry.first_lba,
            num_blocks,
            flags: entry.flags,
        })
    }

    pub fn as_entry(&self) -> format::PartitionTableEntry {
        let mut name = [0u16; 36];
        let raw = self.label.encode_utf16().collect::<Vec<_>>();
        assert!(raw.len() <= name.len());
        name[..raw.len()].copy_from_slice(&raw[..]);
        format::PartitionTableEntry {
            type_guid: self.type_guid.to_bytes(),
            instance_guid: self.instance_guid.to_bytes(),
            first_lba: self.start_block,
            last_lba: self.start_block.saturating_add(self.num_blocks.saturating_sub(1)),
            flags: self.flags,
            name,
        }
    }

    pub fn nil() -> Self {
        Self {
            label: String::default(),
            type_guid: Guid::default(),
            instance_guid: Guid::default(),
            start_block: 0,
            num_blocks: 0,
            flags: 0,
        }
    }

    pub fn is_nil(&self) -> bool {
        self.label == ""
            && self.type_guid.0.is_nil()
            && self.instance_guid.0.is_nil()
            && self.start_block == 0
            && self.num_blocks == 0
            && self.flags == 0
    }
}

impl From<&PartitionInfo> for block_server::PartitionInfo {
    fn from(info: &PartitionInfo) -> Self {
        block_server::PartitionInfo {
            block_count: info.num_blocks,
            start_block_offset: Some(info.start_block),
            type_guid: info.type_guid.to_bytes(),
            instance_guid: info.instance_guid.to_bytes(),
            name: info.label.clone(),
            flags: Some(info.flags),
            ..Default::default()
        }
    }
}

impl From<PartitionInfo> for block_server::PartitionInfo {
    fn from(info: PartitionInfo) -> Self {
        block_server::PartitionInfo::from(&info)
    }
}

enum WhichHeader {
    Primary,
    Backup,
}

impl WhichHeader {
    fn offset(&self, block_size: u64, block_count: u64) -> u64 {
        match self {
            Self::Primary => block_size,
            Self::Backup => (block_count - 1) * block_size,
        }
    }
}

async fn load_metadata(
    client: &RemoteBlockClient,
    which: WhichHeader,
) -> Result<(format::Header, BTreeMap<u32, PartitionInfo>), Error> {
    let bs = client.block_size() as usize;
    let mut header_block = vec![0u8; client.block_size() as usize];
    client
        .read_at(
            MutableBufferSlice::Memory(&mut header_block[..]),
            which.offset(bs as u64, client.block_count() as u64),
        )
        .await
        .context("Read header")?;
    let (header, _) = format::Header::ref_from_prefix(&header_block[..])
        .map_err(|_| anyhow!("Header has invalid size"))?;
    header.ensure_integrity(client.block_count(), client.block_size() as u64)?;
    let partition_table_offset = header.part_start * bs as u64;
    let partition_table_size = (header.num_parts * header.part_size) as usize;
    let partition_table_size_rounded =
        partition_table_size.checked_next_multiple_of(bs).ok_or_else(|| {
            anyhow!(
                "Overflow when rounding up partition table size \
                 (partition_table_size: {partition_table_size}, block_size: {bs})"
            )
        })?;
    let mut partition_table = BTreeMap::new();
    if header.num_parts > 0 {
        let mut partition_table_blocks = vec![0u8; partition_table_size_rounded];
        client
            .read_at(
                MutableBufferSlice::Memory(&mut partition_table_blocks[..]),
                partition_table_offset,
            )
            .await
            .with_context(|| {
                format!(
                    "Failed to read partition table (sz {}) from offset {}",
                    partition_table_size, partition_table_offset
                )
            })?;
        let crc = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC)
            .checksum(&partition_table_blocks[..partition_table_size]);
        ensure!(header.crc32_parts == crc, "Invalid partition table checksum");

        let mut used_ranges = Vec::new();
        for i in 0..header.num_parts as usize {
            let entry_raw = &partition_table_blocks
                [i * header.part_size as usize..(i + 1) * header.part_size as usize];
            let (entry, _) = format::PartitionTableEntry::ref_from_prefix(entry_raw)
                .map_err(|_| anyhow!("Failed to parse partition {i}"))?;
            if entry.is_empty() {
                continue;
            }
            entry
                .ensure_integrity(header.first_usable, header.last_usable)
                .context("GPT partition table entry invalid!")?;
            let end = entry.last_lba.checked_add(1).ok_or_else(|| {
                anyhow!(
                    "Partition {i} last_lba overflow (first_lba: {}, last_lba: {})",
                    entry.first_lba,
                    entry.last_lba
                )
            })?;
            used_ranges.push(entry.first_lba..end);

            partition_table.insert(i as u32, PartitionInfo::from_entry(entry)?);
        }
        used_ranges.sort_by_key(|r| r.start);
        for pairs in used_ranges.windows(2) {
            ensure!(pairs[0].end <= pairs[1].start, "Overlapping partitions");
        }
    }
    Ok((header.clone(), partition_table))
}

struct TransactionState {
    pending_id: u64,
    next_id: u64,
}

impl Default for TransactionState {
    fn default() -> Self {
        Self { pending_id: u64::MAX, next_id: 0 }
    }
}

/// Manages a connection to a GPT-formatted block device.
pub struct Gpt {
    client: Arc<RemoteBlockClient>,
    header: format::Header,
    partitions: BTreeMap<u32, PartitionInfo>,
    transaction_state: Arc<Mutex<TransactionState>>,
}

impl std::fmt::Debug for Gpt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("Gpt")
            .field("header", &self.header)
            .field("partitions", &self.partitions)
            .finish()
    }
}

#[derive(Eq, thiserror::Error, Clone, Debug, PartialEq)]
pub enum TransactionCommitError {
    #[error("I/O error")]
    Io,
    #[error("Invalid arguments")]
    InvalidArguments,
    #[error("No space")]
    NoSpace,
}

impl From<format::FormatError> for TransactionCommitError {
    fn from(error: format::FormatError) -> Self {
        match error {
            format::FormatError::InvalidArguments => Self::InvalidArguments,
            format::FormatError::NoSpace => Self::NoSpace,
        }
    }
}

impl From<TransactionCommitError> for zx::Status {
    fn from(error: TransactionCommitError) -> zx::Status {
        match error {
            TransactionCommitError::Io => zx::Status::IO,
            TransactionCommitError::InvalidArguments => zx::Status::INVALID_ARGS,
            TransactionCommitError::NoSpace => zx::Status::NO_SPACE,
        }
    }
}

#[derive(Eq, thiserror::Error, Clone, Debug, PartialEq)]
pub enum AddPartitionError {
    #[error("Invalid arguments")]
    InvalidArguments,
    #[error("No space")]
    NoSpace,
}

impl From<AddPartitionError> for zx::Status {
    fn from(error: AddPartitionError) -> zx::Status {
        match error {
            AddPartitionError::InvalidArguments => zx::Status::INVALID_ARGS,
            AddPartitionError::NoSpace => zx::Status::NO_SPACE,
        }
    }
}

impl Gpt {
    /// Loads and validates a GPT-formatted block device.
    pub async fn open(client: Arc<RemoteBlockClient>) -> Result<Self, Error> {
        let mut restore_primary = false;
        let (header, partitions) = match load_metadata(&client, WhichHeader::Primary).await {
            Ok(v) => v,
            Err(error) => {
                log::warn!(error:?; "Failed to load primary metadata; falling back to backup");
                restore_primary = true;
                load_metadata(&client, WhichHeader::Backup)
                    .await
                    .context("Failed to load backup metadata")?
            }
        };
        let mut this = Self {
            client,
            header,
            partitions,
            transaction_state: Arc::new(Mutex::new(TransactionState::default())),
        };
        if restore_primary {
            log::info!("Restoring primary metadata from backup!");
            this.header.backup_lba = this.header.current_lba;
            this.header.current_lba = 1;
            this.header.part_start = 2;
            this.header.crc32 = this.header.compute_checksum();
            let partition_table =
                this.flattened_partitions().into_iter().map(|v| v.as_entry()).collect::<Vec<_>>();
            let partition_table_raw = format::serialize_partition_table(
                &mut this.header,
                this.client.block_size() as usize,
                this.client.block_count(),
                &partition_table[..],
            )
            .context("Failed to serialize existing partition table")?;
            this.write_metadata(&this.header, &partition_table_raw[..])
                .await
                .context("Failed to restore primary metadata")?;
        }
        Ok(this)
    }

    /// Formats `client` as a new GPT with `partitions`.  Overwrites any existing GPT on the block
    /// device.
    pub async fn format(
        client: Arc<RemoteBlockClient>,
        partitions: Vec<PartitionInfo>,
    ) -> Result<Self, Error> {
        let header = format::Header::new(
            client.block_count(),
            client.block_size(),
            partitions.len() as u32,
        )?;
        let mut this = Self {
            client,
            header,
            partitions: BTreeMap::new(),
            transaction_state: Arc::new(Mutex::new(TransactionState::default())),
        };
        let mut transaction = this.create_transaction().unwrap();
        transaction.partitions = partitions;
        this.commit_transaction(transaction).await?;
        Ok(this)
    }

    pub fn client(&self) -> &Arc<RemoteBlockClient> {
        &self.client
    }

    /// Replaces the underlying block client. Note that this drops the existing client and so will
    /// cause cancelled futures if any requests are already in-flight.
    pub fn set_client(&mut self, client: Arc<RemoteBlockClient>) {
        self.client = client;
    }

    #[cfg(test)]
    fn take_client(self) -> Arc<RemoteBlockClient> {
        self.client
    }

    pub fn header(&self) -> &format::Header {
        &self.header
    }

    pub fn partitions(&self) -> &BTreeMap<u32, PartitionInfo> {
        &self.partitions
    }

    // We only store valid partitions in memory.  This function allows us to flatten this back out
    // to a non-sparse array for serialization.
    fn flattened_partitions(&self) -> Vec<PartitionInfo> {
        let mut partitions = vec![PartitionInfo::nil(); self.header.num_parts as usize];
        for (idx, partition) in &self.partitions {
            partitions[*idx as usize] = partition.clone();
        }
        partitions
    }

    /// Returns None if there's already a pending transaction.
    pub fn create_transaction(&self) -> Option<Transaction> {
        {
            let mut state = self.transaction_state.lock();
            if state.pending_id != u64::MAX {
                return None;
            } else {
                state.pending_id = state.next_id;
                state.next_id += 1;
            }
        }
        Some(Transaction {
            partitions: self.flattened_partitions(),
            transaction_state: self.transaction_state.clone(),
        })
    }

    pub async fn commit_transaction(
        &mut self,
        mut transaction: Transaction,
    ) -> Result<(), TransactionCommitError> {
        let mut new_header = self.header.clone();
        let entries =
            transaction.partitions.iter().map(|entry| entry.as_entry()).collect::<Vec<_>>();
        let partition_table_raw = format::serialize_partition_table(
            &mut new_header,
            self.client.block_size() as usize,
            self.client.block_count(),
            &entries[..],
        )?;

        let mut backup_header = new_header.clone();
        backup_header.current_lba = backup_header.backup_lba;
        backup_header.backup_lba = 1;
        backup_header.part_start = backup_header.last_usable + 1;
        backup_header.crc32 = backup_header.compute_checksum();

        // Per section 5.3.2 of the UEFI spec, the backup metadata must be written first.  The spec
        // permits the partition table entries and header to be written in either order.
        self.write_metadata(&backup_header, &partition_table_raw[..]).await.map_err(|err| {
            log::warn!(err:?; "Failed to write metadata");
            TransactionCommitError::Io
        })?;
        // NB: It would be preferable to use a barrier here, but not all drivers support barriers at
        // this time.
        // TODO(https://fxbug.dev/416348380): Use a barrier between writing secondary/primary.
        self.client.flush().await.map_err(|err| {
            log::warn!(err:?; "Failed to flush metadata writes");
            TransactionCommitError::Io
        })?;
        self.write_metadata(&new_header, &partition_table_raw[..]).await.map_err(|err| {
            log::warn!(err:?; "Failed to write metadata");
            TransactionCommitError::Io
        })?;
        self.client.flush().await.map_err(|err| {
            log::warn!(err:?; "Failed to flush metadata writes");
            TransactionCommitError::Io
        })?;

        self.header = new_header;
        self.partitions = BTreeMap::new();
        let mut idx = 0;
        for partition in std::mem::take(&mut transaction.partitions) {
            if !partition.is_nil() {
                self.partitions.insert(idx, partition);
            }
            idx += 1;
        }
        Ok(())
    }

    /// Adds a partition in `transaction`.  `info.start_block` must be unset and will be dynamically
    /// chosen in a first-fit manner.
    /// The indedx of the partition in the table is returned on success.
    pub fn add_partition(
        &mut self,
        transaction: &mut Transaction,
        mut info: PartitionInfo,
    ) -> Result<usize, AddPartitionError> {
        assert_eq!(info.start_block, 0);

        if info.label.is_empty()
            || info.type_guid.0.is_nil()
            || info.instance_guid.0.is_nil()
            || info.num_blocks == 0
        {
            return Err(AddPartitionError::InvalidArguments);
        }

        let last_usable_end =
            self.header.last_usable.checked_add(1).ok_or(AddPartitionError::InvalidArguments)?;
        let mut allocated_ranges =
            vec![0..self.header.first_usable, last_usable_end..self.client.block_count()];
        let mut slot_idx = None;
        for i in 0..transaction.partitions.len() {
            let partition = &transaction.partitions[i];
            if slot_idx.is_none() && partition.is_nil() {
                slot_idx = Some(i);
            }
            if !partition.is_nil() {
                let end = partition
                    .start_block
                    .checked_add(partition.num_blocks)
                    .ok_or(AddPartitionError::InvalidArguments)?;
                allocated_ranges.push(partition.start_block..end);
            }
        }
        let slot_idx = slot_idx.ok_or(AddPartitionError::NoSpace)?;
        allocated_ranges.sort_by_key(|range| range.start);

        let alignment = (4096 / self.client.block_size() as u64).max(1);
        let mut start_block = None;
        for [a, b] in allocated_ranges.array_windows() {
            let candidate = a.end.next_multiple_of(alignment);
            if candidate <= b.start && b.start - candidate >= info.num_blocks {
                start_block = Some(candidate);
                break;
            }
        }
        info.start_block = start_block.ok_or(AddPartitionError::NoSpace)?;

        transaction.partitions[slot_idx] = info;
        Ok(slot_idx)
    }

    async fn write_metadata(
        &self,
        header: &format::Header,
        partition_table: &[u8],
    ) -> Result<(), Error> {
        let bs = self.client.block_size() as usize;
        let mut header_block = vec![0u8; bs];
        header.write_to_prefix(&mut header_block[..]).unwrap();
        self.client
            .write_at(BufferSlice::Memory(&header_block[..]), header.current_lba * bs as u64)
            .await
            .context("Failed to write header")?;
        if !partition_table.is_empty() {
            self.client
                .write_at(BufferSlice::Memory(partition_table), header.part_start * bs as u64)
                .await
                .context("Failed to write partition table")?;
        }
        Ok(())
    }
}

/// Pending changes to the GPT.
pub struct Transaction {
    pub partitions: Vec<PartitionInfo>,
    transaction_state: Arc<Mutex<TransactionState>>,
}

impl std::fmt::Debug for Transaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("Transaction").field("partitions", &self.partitions).finish()
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        let mut state = self.transaction_state.lock();
        debug_assert!(state.pending_id != u64::MAX);
        state.pending_id = u64::MAX;
    }
}

#[cfg(test)]
mod tests {
    use crate::{AddPartitionError, Gpt, Guid, PartitionInfo, format};
    use anyhow::Error;
    use block_client::{BlockClient as _, BufferSlice, MutableBufferSlice, RemoteBlockClient};
    use fidl_fuchsia_storage_block as fblock;
    use fuchsia_async as fasync;
    use std::ops::Range;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_vmo_backed_block_server::{
        InitialContents, Observer, VmoBackedServer, VmoBackedServerOptions, WriteAction, WriteCache,
    };
    use zerocopy::IntoBytes as _;

    async fn connect_to_server(
        server: VmoBackedServer,
    ) -> (Arc<RemoteBlockClient>, fasync::Task<()>) {
        let (client, server_end) = fidl::endpoints::create_proxy::<fblock::BlockMarker>();
        let task =
            fasync::Task::spawn(
                async move { server.serve(server_end.into_stream()).await.unwrap() },
            );
        let client = Arc::new(RemoteBlockClient::new(client).await.unwrap());
        (client, task)
    }

    #[fuchsia::test]
    async fn load_unformatted_gpt() {
        let server = VmoBackedServer::new(8, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::open(client).await.expect_err("load should fail");
    }

    #[fuchsia::test]
    async fn load_formatted_empty_gpt() {
        let server = VmoBackedServer::new(8, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(client.clone(), vec![]).await.expect("format failed");
        Gpt::open(client).await.expect("load should succeed");
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_minimal_size() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part";

        let server = VmoBackedServer::new(6, 4096, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 3,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let manager = Gpt::open(client).await.expect("load should succeed");
        assert_eq!(manager.header.first_usable, 3);
        assert_eq!(manager.header.last_usable, 3);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.start_block, 3);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&1).is_none());
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_one_partition() {
        const PART_TYPE_GUID: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        const PART_INSTANCE_GUID: [u8; 16] =
            [16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31];
        const PART_NAME: &str = "part";

        let server = VmoBackedServer::new(8, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let manager = Gpt::open(client).await.expect("load should succeed");
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, "part");
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_GUID);
        assert_eq!(partition.start_block, 4);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&1).is_none());
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_two_partitions() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let server = VmoBackedServer::new(16, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![
                PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                },
                PartitionInfo {
                    label: PART_2_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_2_GUID),
                    start_block: 7,
                    num_blocks: 1,
                    flags: 0,
                },
            ],
        )
        .await
        .expect("format failed");
        let manager = Gpt::open(client).await.expect("load should succeed");
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_1_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_1_GUID);
        assert_eq!(partition.start_block, 4);
        assert_eq!(partition.num_blocks, 1);
        let partition = manager.partitions().get(&1).expect("No entry found");
        assert_eq!(partition.label, PART_2_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_2_GUID);
        assert_eq!(partition.start_block, 7);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&2).is_none());
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_extra_bytes_in_partition_name() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part\0extrastuff";

        let server = VmoBackedServer::new(8, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let manager = Gpt::open(client).await.expect("load should succeed");
        let partition = manager.partitions().get(&0).expect("No entry found");
        // The name should have everything after the first nul byte stripped.
        assert_eq!(partition.label, "part");
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_empty_partition_name() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "";

        let server = VmoBackedServer::new(8, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let manager = Gpt::open(client).await.expect("load should succeed");
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, "");
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_invalid_primary_header() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let server = VmoBackedServer::new(16, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![
                PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                },
                PartitionInfo {
                    label: PART_2_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_2_GUID),
                    start_block: 7,
                    num_blocks: 1,
                    flags: 0,
                },
            ],
        )
        .await
        .expect("format failed");
        // Clobber the primary header.  The backup should allow the GPT to be used.
        client.write_at(BufferSlice::Memory(&[0xffu8; 512]), 512).await.unwrap();
        let manager = Gpt::open(client).await.expect("load should succeed");
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_1_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_1_GUID);
        assert_eq!(partition.start_block, 4);
        assert_eq!(partition.num_blocks, 1);
        let partition = manager.partitions().get(&1).expect("No entry found");
        assert_eq!(partition.label, PART_2_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_2_GUID);
        assert_eq!(partition.start_block, 7);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&2).is_none());
    }

    #[fuchsia::test]
    async fn load_formatted_gpt_with_invalid_primary_partition_table() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let server = VmoBackedServer::new(16, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![
                PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                },
                PartitionInfo {
                    label: PART_2_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_2_GUID),
                    start_block: 7,
                    num_blocks: 1,
                    flags: 0,
                },
            ],
        )
        .await
        .expect("format failed");
        // Clobber the primary partition table.  The backup should allow the GPT to be used.
        client.write_at(BufferSlice::Memory(&[0xffu8; 512]), 1024).await.unwrap();
        let manager = Gpt::open(client).await.expect("load should succeed");
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_1_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_1_GUID);
        assert_eq!(partition.start_block, 4);
        assert_eq!(partition.num_blocks, 1);
        let partition = manager.partitions().get(&1).expect("No entry found");
        assert_eq!(partition.label, PART_2_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_2_GUID);
        assert_eq!(partition.start_block, 7);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&2).is_none());
    }

    #[fuchsia::test]
    async fn drop_transaction() {
        let server = VmoBackedServer::new(16, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(client.clone(), vec![]).await.expect("format failed");
        let manager = Gpt::open(client).await.expect("load should succeed");
        {
            let _transaction = manager.create_transaction().unwrap();
            assert!(manager.create_transaction().is_none());
        }
        let _transaction =
            manager.create_transaction().expect("Transaction dropped but not available");
    }

    #[fuchsia::test]
    async fn commit_empty_transaction() {
        let server = VmoBackedServer::new(16, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(client.clone(), vec![]).await.expect("format failed");
        let mut manager = Gpt::open(client).await.expect("load should succeed");
        let transaction = manager.create_transaction().unwrap();
        manager.commit_transaction(transaction).await.expect("Commit failed");

        // Check state before and after a reload, to ensure both the in-memory and on-disk
        // representation match.
        assert_eq!(manager.header().num_parts, 0);
        assert!(manager.partitions().is_empty());
        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 0);
        assert!(manager.partitions().is_empty());
    }

    #[fuchsia::test]
    async fn add_partition_in_transaction() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let server = VmoBackedServer::new(16, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: PART_1_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let mut manager = Gpt::open(client).await.expect("load should succeed");
        let mut transaction = manager.create_transaction().unwrap();
        assert_eq!(transaction.partitions.len(), 1);
        transaction.partitions.push(crate::PartitionInfo {
            label: PART_2_NAME.to_string(),
            type_guid: crate::Guid::from_bytes(PART_TYPE_GUID),
            instance_guid: crate::Guid::from_bytes(PART_INSTANCE_2_GUID),
            start_block: 7,
            num_blocks: 1,
            flags: 0,
        });
        manager.commit_transaction(transaction).await.expect("Commit failed");

        // Check state before and after a reload, to ensure both the in-memory and on-disk
        // representation match.
        assert_eq!(manager.header().num_parts, 2);
        assert!(manager.partitions().get(&2).is_none());
        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 2);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_1_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_1_GUID);
        assert_eq!(partition.start_block, 4);
        assert_eq!(partition.num_blocks, 1);
        let partition = manager.partitions().get(&1).expect("No entry found");
        assert_eq!(partition.label, PART_2_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_2_GUID);
        assert_eq!(partition.start_block, 7);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&2).is_none());
    }

    #[fuchsia::test]
    async fn remove_partition_in_transaction() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part1";

        let server = VmoBackedServer::new(16, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let mut manager = Gpt::open(client).await.expect("load should succeed");
        let mut transaction = manager.create_transaction().unwrap();
        assert_eq!(transaction.partitions.len(), 1);
        transaction.partitions.clear();
        manager.commit_transaction(transaction).await.expect("Commit failed");

        // Check state before and after a reload, to ensure both the in-memory and on-disk
        // representation match.
        assert_eq!(manager.header().num_parts, 0);
        assert!(manager.partitions().get(&0).is_none());
        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 0);
        assert!(manager.partitions().get(&0).is_none());
    }

    #[fuchsia::test]
    async fn modify_partition_in_transaction() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let server = VmoBackedServer::new(16, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: PART_1_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let mut manager = Gpt::open(client).await.expect("load should succeed");
        let mut transaction = manager.create_transaction().unwrap();
        assert_eq!(transaction.partitions.len(), 1);
        transaction.partitions[0] = crate::PartitionInfo {
            label: PART_2_NAME.to_string(),
            type_guid: crate::Guid::from_bytes(PART_TYPE_GUID),
            instance_guid: crate::Guid::from_bytes(PART_INSTANCE_2_GUID),
            start_block: 7,
            num_blocks: 1,
            flags: 0,
        };
        manager.commit_transaction(transaction).await.expect("Commit failed");

        // Check state before and after a reload, to ensure both the in-memory and on-disk
        // representation match.
        assert_eq!(manager.header().num_parts, 1);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_2_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_2_GUID);
        assert_eq!(partition.start_block, 7);
        assert_eq!(partition.num_blocks, 1);
        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 1);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_2_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_2_GUID);
        assert_eq!(partition.start_block, 7);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&1).is_none());
    }

    #[fuchsia::test]
    async fn grow_partition_table_in_transaction() {
        let server =
            VmoBackedServer::new(2048, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: "part".to_string(),
                type_guid: Guid::from_bytes([1u8; 16]),
                instance_guid: Guid::from_bytes([1u8; 16]),
                start_block: 34,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let mut manager = Gpt::open(client).await.expect("load should succeed");
        assert_eq!(manager.header().num_parts, 1);
        assert_eq!(manager.header().first_usable, 3);
        let mut transaction = manager.create_transaction().unwrap();
        transaction.partitions.resize(128, crate::PartitionInfo::nil());
        manager.commit_transaction(transaction).await.expect("Commit failed");

        // Check state before and after a reload, to ensure both the in-memory and on-disk
        // representation match.
        assert_eq!(manager.header().num_parts, 128);
        assert_eq!(manager.header().first_usable, 34);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, "part");
        assert_eq!(partition.type_guid.to_bytes(), [1u8; 16]);
        assert_eq!(partition.instance_guid.to_bytes(), [1u8; 16]);
        assert_eq!(partition.start_block, 34);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&1).is_none());
        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 128);
        assert_eq!(manager.header().first_usable, 34);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, "part");
        assert_eq!(partition.type_guid.to_bytes(), [1u8; 16]);
        assert_eq!(partition.instance_guid.to_bytes(), [1u8; 16]);
        assert_eq!(partition.start_block, 34);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&1).is_none());
    }

    #[fuchsia::test]
    async fn shrink_partition_table_in_transaction() {
        let mut partitions = vec![];
        for i in 0..128 {
            partitions.push(PartitionInfo {
                label: format!("part-{i}"),
                type_guid: Guid::from_bytes([i as u8 + 1; 16]),
                instance_guid: Guid::from_bytes([i as u8 + 1; 16]),
                start_block: 34 + i,
                num_blocks: 1,
                flags: 0,
            });
        }
        let server =
            VmoBackedServer::new(2048, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(client.clone(), partitions).await.expect("format failed");
        let mut manager = Gpt::open(client).await.expect("load should succeed");
        assert_eq!(manager.header().num_parts, 128);
        assert_eq!(manager.header().first_usable, 34);
        let mut transaction = manager.create_transaction().unwrap();
        transaction.partitions.clear();
        manager.commit_transaction(transaction).await.expect("Commit failed");

        // Check state before and after a reload, to ensure both the in-memory and on-disk
        // representation match.
        assert_eq!(manager.header().num_parts, 0);
        assert_eq!(manager.header().first_usable, 2);
        assert!(manager.partitions().get(&0).is_none());
        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 0);
        assert_eq!(manager.header().first_usable, 2);
        assert!(manager.partitions().get(&0).is_none());
    }

    #[fuchsia::test]
    async fn invalid_transaction_rejected() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part1";

        let server = VmoBackedServer::new(16, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let mut manager = Gpt::open(client).await.expect("load should succeed");
        let mut transaction = manager.create_transaction().unwrap();
        assert_eq!(transaction.partitions.len(), 1);
        // This overlaps with the GPT metadata, so is invalid.
        transaction.partitions[0].start_block = 0;
        manager.commit_transaction(transaction).await.expect_err("Commit should have failed");

        // Ensure nothing changed. Check state before and after a reload, to ensure both the
        // in-memory and on-disk representation match.
        assert_eq!(manager.header().num_parts, 1);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_GUID);
        assert_eq!(partition.start_block, 4);
        assert_eq!(partition.num_blocks, 1);
        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 1);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_GUID);
        assert_eq!(partition.start_block, 4);
        assert_eq!(partition.num_blocks, 1);
    }

    /// An Observer that discards all writes overlapping its range (specified in bytes, not blocks).
    struct DiscardingObserver {
        block_size: u64,
        discard_range: Range<u64>,
    }

    impl Observer for DiscardingObserver {
        fn write(
            &self,
            device_block_offset: u64,
            block_count: u32,
            _vmo: &Arc<zx::Vmo>,
            _vmo_offset: u64,
            _opts: block_server::WriteOptions,
        ) -> WriteAction {
            let write_range = (device_block_offset * self.block_size)
                ..(device_block_offset + block_count as u64) * self.block_size;
            if write_range.end <= self.discard_range.start
                || write_range.start >= self.discard_range.end
            {
                WriteAction::Write
            } else {
                WriteAction::Discard
            }
        }
    }

    #[fuchsia::test]
    async fn transaction_applied_if_primary_metadata_partially_written() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let vmo = zx::Vmo::create(8192).unwrap();
        let server = VmoBackedServerOptions {
            initial_contents: InitialContents::FromVmo(vmo),
            block_size: 512,
            observer: Some(Box::new(DiscardingObserver {
                discard_range: 1024..1536,
                block_size: 512,
            })),
            ..Default::default()
        }
        .build()
        .unwrap();
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: PART_1_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let mut manager = Gpt::open(client).await.expect("load should succeed");
        let mut transaction = manager.create_transaction().unwrap();
        transaction.partitions.push(crate::PartitionInfo {
            label: PART_2_NAME.to_string(),
            type_guid: crate::Guid::from_bytes(PART_TYPE_GUID),
            instance_guid: crate::Guid::from_bytes(PART_INSTANCE_2_GUID),
            start_block: 7,
            num_blocks: 1,
            flags: 0,
        });
        manager.commit_transaction(transaction).await.expect("Commit failed");

        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 2);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_1_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_1_GUID);
        assert_eq!(partition.start_block, 4);
        assert_eq!(partition.num_blocks, 1);
        let partition = manager.partitions().get(&1).expect("No entry found");
        assert_eq!(partition.label, PART_2_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_2_GUID);
        assert_eq!(partition.start_block, 7);
        assert_eq!(partition.num_blocks, 1);
    }

    #[fuchsia::test]
    async fn transaction_not_applied_if_primary_metadata_not_written() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let vmo = zx::Vmo::create(8192).unwrap();
        let vmo_dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        {
            let server =
                VmoBackedServer::from_vmo(512, vmo_dup).expect("Failed to create VmoBackedServer");
            let (client, _task) = connect_to_server(server).await;
            Gpt::format(
                client.clone(),
                vec![PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                }],
            )
            .await
            .expect("format failed");
        }
        let server = VmoBackedServerOptions {
            initial_contents: InitialContents::FromVmo(vmo),
            block_size: 512,
            observer: Some(Box::new(DiscardingObserver {
                discard_range: 0..2048,
                block_size: 512,
            })),
            ..Default::default()
        }
        .build()
        .unwrap();
        let (client, _task) = connect_to_server(server).await;

        let mut manager = Gpt::open(client).await.expect("load should succeed");
        let mut transaction = manager.create_transaction().unwrap();
        transaction.partitions.push(crate::PartitionInfo {
            label: PART_2_NAME.to_string(),
            type_guid: crate::Guid::from_bytes(PART_TYPE_GUID),
            instance_guid: crate::Guid::from_bytes(PART_INSTANCE_2_GUID),
            start_block: 7,
            num_blocks: 1,
            flags: 0,
        });
        manager.commit_transaction(transaction).await.expect("Commit failed");

        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 1);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_1_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_1_GUID);
        assert_eq!(partition.start_block, 4);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&1).is_none());
    }

    #[fuchsia::test]
    async fn transaction_not_applied_if_backup_metadata_partially_written() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_1_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_2_GUID: [u8; 16] = [3u8; 16];
        const PART_1_NAME: &str = "part1";
        const PART_2_NAME: &str = "part2";

        let vmo = zx::Vmo::create(8192).unwrap();
        let vmo_dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        {
            let server =
                VmoBackedServer::from_vmo(512, vmo_dup).expect("Failed to create VmoBackedServer");
            let (client, _task) = connect_to_server(server).await;
            Gpt::format(
                client.clone(),
                vec![PartitionInfo {
                    label: PART_1_NAME.to_string(),
                    type_guid: Guid::from_bytes(PART_TYPE_GUID),
                    instance_guid: Guid::from_bytes(PART_INSTANCE_1_GUID),
                    start_block: 4,
                    num_blocks: 1,
                    flags: 0,
                }],
            )
            .await
            .expect("format failed");
        }
        let server = VmoBackedServerOptions {
            initial_contents: InitialContents::FromVmo(vmo),
            block_size: 512,
            observer: Some(Box::new(DiscardingObserver {
                discard_range: 0..7680,
                block_size: 512,
            })),
            ..Default::default()
        }
        .build()
        .unwrap();
        let (client, _task) = connect_to_server(server).await;

        let mut manager = Gpt::open(client).await.expect("load should succeed");
        let mut transaction = manager.create_transaction().unwrap();
        transaction.partitions.push(crate::PartitionInfo {
            label: PART_2_NAME.to_string(),
            type_guid: crate::Guid::from_bytes(PART_TYPE_GUID),
            instance_guid: crate::Guid::from_bytes(PART_INSTANCE_2_GUID),
            start_block: 7,
            num_blocks: 1,
            flags: 0,
        });
        manager.commit_transaction(transaction).await.expect("Commit failed");

        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 1);
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, PART_1_NAME);
        assert_eq!(partition.type_guid.to_bytes(), PART_TYPE_GUID);
        assert_eq!(partition.instance_guid.to_bytes(), PART_INSTANCE_1_GUID);
        assert_eq!(partition.start_block, 4);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&1).is_none());
    }

    #[fuchsia::test]
    async fn restore_primary_from_backup() {
        const PART_TYPE_GUID: [u8; 16] = [2u8; 16];
        const PART_INSTANCE_GUID: [u8; 16] = [2u8; 16];
        const PART_NAME: &str = "part1";

        let server = VmoBackedServer::new(16, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(
            client.clone(),
            vec![PartitionInfo {
                label: PART_NAME.to_string(),
                type_guid: Guid::from_bytes(PART_TYPE_GUID),
                instance_guid: Guid::from_bytes(PART_INSTANCE_GUID),
                start_block: 4,
                num_blocks: 1,
                flags: 0,
            }],
        )
        .await
        .expect("format failed");
        let mut old_metadata = vec![0u8; 2048];
        client.read_at(MutableBufferSlice::Memory(&mut old_metadata[..]), 0).await.unwrap();
        let mut buffer = vec![0u8; 2048];
        client.write_at(BufferSlice::Memory(&buffer[..]), 0).await.unwrap();

        let manager = Gpt::open(client).await.expect("load should succeed");
        let client = manager.take_client();

        client.read_at(MutableBufferSlice::Memory(&mut buffer[..]), 0).await.unwrap();
        assert_eq!(old_metadata, buffer);
    }

    #[fuchsia::test]
    async fn load_golden_gpt_linux() {
        let server = VmoBackedServer::from_file(512, "/pkg/data/gpt_golden/gpt.linux.blk");
        let (client, _task) = connect_to_server(server).await;
        let manager = Gpt::open(client).await.expect("load should succeed");
        let partition = manager.partitions().get(&0).expect("No entry found");
        assert_eq!(partition.label, "ext");
        assert_eq!(partition.type_guid.to_string(), "0fc63daf-8483-4772-8e79-3d69d8477de4");
        assert_eq!(partition.start_block, 8);
        assert_eq!(partition.num_blocks, 1);
        assert!(manager.partitions().get(&1).is_none());
    }

    #[fuchsia::test]
    async fn load_golden_gpt_fuchsia() {
        let server = VmoBackedServer::from_file(512, "/pkg/data/gpt_golden/gpt.fuchsia.blk");
        let (client, _task) = connect_to_server(server).await;

        struct ExpectedPartition {
            label: &'static str,
            type_guid: &'static str,
            blocks: Range<u64>,
        }
        const EXPECTED_PARTITIONS: [ExpectedPartition; 8] = [
            ExpectedPartition {
                label: "bootloader",
                type_guid: "5ece94fe-4c86-11e8-a15b-480fcf35f8e6",
                blocks: 11..12,
            },
            ExpectedPartition {
                label: "zircon_a",
                type_guid: "9b37fff6-2e58-466a-983a-f7926d0b04e0",
                blocks: 12..13,
            },
            ExpectedPartition {
                label: "zircon_b",
                type_guid: "9b37fff6-2e58-466a-983a-f7926d0b04e0",
                blocks: 13..14,
            },
            ExpectedPartition {
                label: "zircon_r",
                type_guid: "9b37fff6-2e58-466a-983a-f7926d0b04e0",
                blocks: 14..15,
            },
            ExpectedPartition {
                label: "vbmeta_a",
                type_guid: "421a8bfc-85d9-4d85-acda-b64eec0133e9",
                blocks: 15..16,
            },
            ExpectedPartition {
                label: "vbmeta_b",
                type_guid: "421a8bfc-85d9-4d85-acda-b64eec0133e9",
                blocks: 16..17,
            },
            ExpectedPartition {
                label: "vbmeta_r",
                type_guid: "421a8bfc-85d9-4d85-acda-b64eec0133e9",
                blocks: 17..18,
            },
            ExpectedPartition {
                label: "durable_boot",
                type_guid: "a409e16b-78aa-4acc-995c-302352621a41",
                blocks: 18..19,
            },
        ];

        let manager = Gpt::open(client).await.expect("load should succeed");
        for i in 0..EXPECTED_PARTITIONS.len() as u32 {
            let partition = manager.partitions().get(&i).expect("No entry found");
            let expected = &EXPECTED_PARTITIONS[i as usize];
            assert_eq!(partition.label, expected.label);
            assert_eq!(partition.type_guid.to_string(), expected.type_guid);
            assert_eq!(partition.start_block, expected.blocks.start);
            assert_eq!(partition.num_blocks, expected.blocks.end - expected.blocks.start);
        }
    }

    #[fuchsia::test]
    async fn add_partitions_till_no_blocks_left() {
        let server = VmoBackedServer::new(128, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(client.clone(), vec![PartitionInfo::nil(); 32]).await.expect("format failed");
        let mut manager = Gpt::open(client).await.expect("load should succeed");
        let mut transaction = manager.create_transaction().unwrap();
        assert_eq!(transaction.partitions.len(), 32);
        let mut num = 0;
        loop {
            match manager.add_partition(
                &mut transaction,
                crate::PartitionInfo {
                    label: format!("part-{num}"),
                    type_guid: crate::Guid::generate(),
                    instance_guid: crate::Guid::generate(),
                    start_block: 0,
                    num_blocks: 1,
                    flags: 0,
                },
            ) {
                Ok(_) => {
                    num += 1;
                }
                Err(AddPartitionError::InvalidArguments) => panic!("Unexpected error"),
                Err(AddPartitionError::NoSpace) => break,
            };
        }
        assert!(num <= 32);
        manager.commit_transaction(transaction).await.expect("Commit failed");

        // Check state before and after a reload, to ensure both the in-memory and on-disk
        // representation match.
        assert_eq!(manager.header().num_parts, 32);
        assert_eq!(manager.partitions().len(), num);

        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 32);
        assert_eq!(manager.partitions().len(), num);
    }

    #[fuchsia::test]
    async fn add_partitions_till_no_slots_left() {
        let server = VmoBackedServer::new(128, 512, &[]).expect("Failed to create VmoBackedServer");
        let (client, _task) = connect_to_server(server).await;
        Gpt::format(client.clone(), vec![PartitionInfo::nil(); 4]).await.expect("format failed");
        let mut manager = Gpt::open(client).await.expect("load should succeed");
        let mut transaction = manager.create_transaction().unwrap();
        assert_eq!(transaction.partitions.len(), 4);
        let mut num = 0;
        loop {
            match manager.add_partition(
                &mut transaction,
                crate::PartitionInfo {
                    label: format!("part-{num}"),
                    type_guid: crate::Guid::generate(),
                    instance_guid: crate::Guid::generate(),
                    start_block: 0,
                    num_blocks: 1,
                    flags: 0,
                },
            ) {
                Ok(_) => {
                    num += 1;
                }
                Err(AddPartitionError::InvalidArguments) => panic!("Unexpected error"),
                Err(AddPartitionError::NoSpace) => break,
            };
        }
        assert!(num <= 4);
        manager.commit_transaction(transaction).await.expect("Commit failed");

        // Check state before and after a reload, to ensure both the in-memory and on-disk
        // representation match.
        assert_eq!(manager.header().num_parts, 4);
        assert_eq!(manager.partitions().len(), num);

        let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
        assert_eq!(manager.header().num_parts, 4);
        assert_eq!(manager.partitions().len(), num);
    }

    /// An Observer that shuffles writes and discards some of the tail since last flush.
    struct ShufflingObserver {
        // Only start shuffling once this is set.
        start: Arc<AtomicBool>,
        // Only shuffle if there is a write to this offset.
        shuffle_if_contains_offset: u64,
    }

    impl Observer for ShufflingObserver {
        fn flush(&self, writes: Option<&mut WriteCache>) {
            if self.start.load(Ordering::Relaxed) {
                let Some(writes) = writes else { unreachable!() };
                if writes
                    .iter()
                    .filter(|(offset, _)| **offset == self.shuffle_if_contains_offset)
                    .next()
                    .is_some()
                {
                    writes.shuffle();
                    writes.discard_some();
                }
            }
        }

        fn close(&self, writes: Option<&mut WriteCache>) {
            // Always shuffle every write which had yet to be flushed when the client closed.
            if self.start.load(Ordering::Relaxed) {
                let Some(writes) = writes else { unreachable!() };
                writes.shuffle();
            }
        }
    }

    #[fuchsia::test]
    async fn metadata_update_is_atomic() {
        const BLOCK_SIZE: u64 = 512;
        const BLOCK_COUNT: u64 = 128;
        // Test once where we shuffle any set of writes which contains the primary superblock, and
        // once where we shuffle any set of writes which contains the secondary superblock.
        // The goal is to ensure that writes are correctly sequenced with some sort of flush or
        // barrier (secondary, <barrier>, primary), so metadata updates are atomic.
        for shuffle_if_contains_offset in [1, BLOCK_COUNT - 1] {
            let vmo = zx::Vmo::create(BLOCK_SIZE * BLOCK_COUNT).unwrap();
            let start_shuffling = Arc::new(AtomicBool::new(false));
            let server = VmoBackedServerOptions {
                initial_contents: InitialContents::FromVmo(vmo),
                block_size: BLOCK_SIZE as u32,
                observer: Some(Box::new(ShufflingObserver {
                    start: start_shuffling.clone(),
                    shuffle_if_contains_offset,
                })),
                write_tracking: true,
                ..Default::default()
            }
            .build()
            .unwrap();
            let (client, _task) = connect_to_server(server).await;
            Gpt::format(client.clone(), vec![PartitionInfo::nil(); 80])
                .await
                .expect("format failed");

            start_shuffling.store(true, Ordering::Relaxed);

            let mut manager = Gpt::open(client).await.expect("load should succeed");
            let mut transaction = manager.create_transaction().unwrap();
            transaction.partitions.truncate(40);
            let mut num = 0;
            loop {
                match manager.add_partition(
                    &mut transaction,
                    crate::PartitionInfo {
                        label: format!("part-{num}"),
                        type_guid: crate::Guid::generate(),
                        instance_guid: crate::Guid::generate(),
                        start_block: 0,
                        num_blocks: 1,
                        flags: 0,
                    },
                ) {
                    Ok(_) => {
                        num += 1;
                    }
                    Err(AddPartitionError::InvalidArguments) => panic!("Unexpected error"),
                    Err(AddPartitionError::NoSpace) => break,
                };
            }
            assert!(num <= 40);
            manager.commit_transaction(transaction).await.expect("Commit failed");

            // Check state before and after a reload.
            assert_eq!(manager.header().num_parts, 40);
            assert_eq!(manager.partitions().len(), num);

            // If the GPT implementation has appropriate barriers/flushes between secondary and
            // primary metadata updates, then we will end up in either the old state or the new
            // state.  Otherwise, both copies might become corrupt and the GPT would be unreadable.
            let manager = Gpt::open(manager.take_client()).await.expect("reload should succeed");
            let len = manager.partitions().len();
            assert!(len == 0 || len == num);
        }
    }

    async fn try_load_invalid_gpt(
        block_count: u64,
        block_size: u32,
        mut header: format::Header,
        entries: Vec<format::PartitionTableEntry>,
    ) -> Result<Gpt, Error> {
        let vmo = zx::Vmo::create(block_count * block_size as u64).unwrap();

        let part_size = std::mem::size_of::<format::PartitionTableEntry>();
        let mut part_table_bytes = vec![0u8; entries.len() * part_size];
        for (i, entry) in entries.iter().enumerate() {
            part_table_bytes[i * part_size..(i + 1) * part_size].copy_from_slice(entry.as_bytes());
        }

        let crc_parts = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&part_table_bytes);
        header.crc32_parts = crc_parts;
        header.crc32 = header.compute_checksum();

        // Write primary
        vmo.write(header.as_bytes(), block_size as u64).unwrap();
        vmo.write(&part_table_bytes, 2 * block_size as u64).unwrap();

        // Write backup
        let mut backup_header = header.clone();
        backup_header.current_lba = block_count - 1;
        backup_header.backup_lba = 1;
        backup_header.part_start = backup_header.last_usable.saturating_add(1);
        backup_header.crc32 = backup_header.compute_checksum();

        vmo.write(backup_header.as_bytes(), (block_count - 1) * block_size as u64).unwrap();

        let partition_table_len = header.part_size as u64 * header.num_parts as u64;
        let partition_table_blocks =
            partition_table_len.checked_next_multiple_of(block_size as u64).unwrap()
                / block_size as u64;

        if let Some(end) = backup_header.part_start.checked_add(partition_table_blocks) {
            if end <= backup_header.current_lba {
                if let Some(offset) = backup_header.part_start.checked_mul(block_size as u64) {
                    vmo.write(&part_table_bytes, offset).unwrap();
                }
            }
        }

        let vmo_clone = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let server = VmoBackedServer::from_vmo(block_size, vmo_clone).unwrap();
        let (client, _task) = connect_to_server(server).await;

        Gpt::open(client).await
    }

    #[fuchsia::test]
    async fn test_partition_before_first_usable() {
        let block_count = 128;
        let block_size = 512;
        let header = format::Header::new(block_count, block_size, 128).unwrap();
        let mut entries = vec![format::PartitionTableEntry::empty(); 128];
        entries[0] = format::PartitionTableEntry {
            type_guid: [1; 16],
            instance_guid: [1; 16],
            first_lba: header.first_usable - 1,
            last_lba: header.first_usable + 10,
            ..format::PartitionTableEntry::empty()
        };
        let res = try_load_invalid_gpt(block_count, block_size, header, entries).await;
        assert!(res.is_err());
        let err_msg = format!("{:?}", res.err().unwrap());
        assert!(
            err_msg.contains("GPT partition table entry invalid"),
            "Unexpected error: {}",
            err_msg
        );
    }

    #[fuchsia::test]
    async fn test_partition_after_last_usable() {
        let block_count = 128;
        let block_size = 512;
        let header = format::Header::new(block_count, block_size, 128).unwrap();
        let mut entries = vec![format::PartitionTableEntry::empty(); 128];
        entries[0] = format::PartitionTableEntry {
            type_guid: [1; 16],
            instance_guid: [1; 16],
            first_lba: header.first_usable,
            last_lba: header.last_usable + 1,
            ..format::PartitionTableEntry::empty()
        };
        let res = try_load_invalid_gpt(block_count, block_size, header, entries).await;
        assert!(res.is_err());
        let err_msg = format!("{:?}", res.err().unwrap());
        assert!(
            err_msg.contains("GPT partition table entry invalid"),
            "Unexpected error: {}",
            err_msg
        );
    }

    #[fuchsia::test]
    async fn test_overlapping_partitions() {
        let block_count = 128;
        let block_size = 512;
        let header = format::Header::new(block_count, block_size, 128).unwrap();
        let mut entries = vec![format::PartitionTableEntry::empty(); 128];
        entries[0] = format::PartitionTableEntry {
            type_guid: [1; 16],
            instance_guid: [1; 16],
            first_lba: header.first_usable,
            last_lba: header.first_usable + 10,
            ..format::PartitionTableEntry::empty()
        };
        entries[1] = format::PartitionTableEntry {
            type_guid: [1; 16],
            instance_guid: [2; 16],
            first_lba: header.first_usable + 5, // Overlaps
            last_lba: header.first_usable + 15,
            ..format::PartitionTableEntry::empty()
        };
        let res = try_load_invalid_gpt(block_count, block_size, header, entries).await;
        assert!(res.is_err());
        let err_msg = format!("{:?}", res.err().unwrap());
        assert!(err_msg.contains("Overlapping partitions"), "Unexpected error: {}", err_msg);
    }

    #[fuchsia::test]
    async fn test_header_first_usable_too_small() {
        let block_count = 128;
        let block_size = 512;
        let mut header = format::Header::new(block_count, block_size, 128).unwrap();
        // partition_table_blocks = 128 * 128 / 512 = 32.
        // first_lba = 1.
        // We want first_usable = first_lba + partition_table_blocks = 33
        // (invalid, overlaps with partition table).
        // Valid first_usable is >= 34.
        header.first_usable = 33;
        let entries = vec![format::PartitionTableEntry::empty(); 128];
        let res = try_load_invalid_gpt(block_count, block_size, header, entries).await;
        assert!(res.is_err());
        let err_msg = format!("{:?}", res.err().unwrap());
        assert!(err_msg.contains("Invalid first_usable"), "Unexpected error: {}", err_msg);
    }

    #[fuchsia::test]
    async fn test_header_last_usable_too_large() {
        let block_count = 128;
        let block_size = 512;
        let mut header = format::Header::new(block_count, block_size, 128).unwrap();
        // partition_table_blocks = 32.
        // second_lba = 127.
        // We want last_usable = 95 (invalid, overlaps with backup partition table starting at 96).
        // Valid last_usable is <= 94.
        header.last_usable = 95;
        let entries = vec![format::PartitionTableEntry::empty(); 128];
        let res = try_load_invalid_gpt(block_count, block_size, header, entries).await;
        assert!(res.is_err());
        let err_msg = format!("{:?}", res.err().unwrap());
        assert!(err_msg.contains("Invalid last_usable"), "Unexpected error: {}", err_msg);
    }

    #[fuchsia::test]
    fn test_from_entry_last_lba_overflow() {
        let entry = format::PartitionTableEntry {
            type_guid: [1; 16],
            instance_guid: [1; 16],
            first_lba: 34,
            last_lba: u64::MAX,
            flags: 0,
            name: [0; 36],
        };
        assert!(PartitionInfo::from_entry(&entry).is_err());
    }

    #[fuchsia::test]
    fn test_from_entry_last_lba_less_than_first_lba() {
        let entry = format::PartitionTableEntry {
            type_guid: [1; 16],
            instance_guid: [1; 16],
            first_lba: 34,
            last_lba: 33,
            flags: 0,
            name: [0; 36],
        };
        assert!(PartitionInfo::from_entry(&entry).is_err());
    }

    #[fuchsia::test]
    async fn test_header_last_usable_overflow() {
        let block_count = 128;
        let block_size = 512;
        let mut header = format::Header::new(block_count, block_size, 128).unwrap();
        header.last_usable = u64::MAX;
        let entries = vec![format::PartitionTableEntry::empty(); 128];
        let res = try_load_invalid_gpt(block_count, block_size, header, entries).await;
        assert!(res.is_err());
    }
}

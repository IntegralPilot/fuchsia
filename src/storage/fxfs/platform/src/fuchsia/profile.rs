// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuchsia::file::FxFile;
use crate::fuchsia::fxblob::BlobDirectory;
use crate::fuchsia::fxblob::blob::FxBlob;
use crate::fuchsia::node::{FxNode, OpenedNode};
use crate::fuchsia::pager::PagerBacked;
use crate::fuchsia::volume::FxVolume;
use anyhow::{Context as _, Error, anyhow, ensure};
use arrayref::{array_refs, mut_array_refs};
use async_trait::async_trait;
use fuchsia_async as fasync;
use fuchsia_hash::Hash;
use futures::future::{self, BoxFuture, join_all};
use futures::lock::Mutex;
use futures::{FutureExt, select};
use fxfs::errors::FxfsError;
use fxfs::log::*;
use fxfs::object_handle::{INVALID_OBJECT_ID, ObjectHandle, ReadObjectHandle, WriteObjectHandle};
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::{
    AttributeId, AttributeKey, DataObjectHandle, HandleOptions, Mutation, ObjectDescriptor,
    ObjectKey, ObjectStore, ObjectValue, Timestamp, VOLUME_DATA_KEY_ID, directory,
};
use linked_hash_map::LinkedHashMap;
use scopeguard::ScopeGuard;
use std::cmp::{Eq, PartialEq};
use std::collections::btree_map::{BTreeMap, Entry};
use std::marker::PhantomData;
use std::mem::size_of;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use storage_device::buffer::{BufferFuture, BufferRef};
use storage_ptr_slice::{MutPtrByteSlice, PtrByteSlice};
use storage_units::PAGE_SIZE;
use vfs::execution_scope::ActiveGuard;

const FILE_OPEN_MARKER: u64 = u64::MAX;
const REPLAY_THREADS: usize = 2;
// The number of messages to buffer before sending to record. They are chunked up to reduce the
// number of allocations in the serving threads.
const MESSAGE_CHUNK_SIZE: usize = 64;
const IO_SIZE: usize = 1 << 17; // 128KiB. Needs to be a power of 2 and >= block size.

pub static RECORDED: AtomicU64 = AtomicU64::new(0);

/// A handle for recording a profile to.
#[async_trait]
pub trait RecordingHandle: Send + Sync {
    /// Append data to the handle.
    async fn append<'a>(&'a self, buf: BufferRef<'a>) -> Result<u64, Error>;

    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_>;

    fn block_size(&self) -> usize;

    /// The recording is finished being appended to the file. Commit it.
    async fn commit(self: Box<Self>) -> Result<(), Error>;

    /// When the recording fails or is stopped prematurely this will be called to clean up the
    /// resources, delete the backing data.
    fn abort_cleanup(self: Box<Self>);

    /// Returns true if events should be filtered based on whether the file was opened during the
    /// recording and encryption settings. For now this is true for FileRecordingHandle and false
    /// for AssociatedRecordingHandle.
    fn filter_events(&self) -> bool;
}

/// For placing the recording in the volume's internal profile directory.
pub struct FileRecordingHandle {
    name: String,
    volume: Arc<FxVolume>,
    handle: DataObjectHandle<FxVolume>,
}

impl FileRecordingHandle {
    pub async fn new(name: &str, volume: Arc<FxVolume>) -> Result<Self, Error> {
        let store = volume.store();
        let mut transaction = store.new_transaction(lock_keys![], Options::default()).await?;
        let handle =
            ObjectStore::create_object(&volume, &mut transaction, HandleOptions::default(), None)
                .await?;
        store.add_to_graveyard(&mut transaction, handle.object_id());
        transaction.commit().await?;

        Ok(Self { name: name.to_string(), volume, handle })
    }

    async fn commit_impl(&self) -> Result<(), Error> {
        let store = self.volume.store();
        let fs = store.filesystem();
        let profile_dir = self.volume.get_profile_directory().await?;

        let mut lock_keys =
            lock_keys![LockKey::object(store.store_object_id(), profile_dir.object_id())];
        let mut old_id = INVALID_OBJECT_ID;
        let mut transaction = loop {
            let transaction = store.new_transaction(lock_keys, Options::default()).await?;
            if let Some((id, descriptor, _)) = profile_dir.lookup(&self.name).await? {
                ensure!(matches!(descriptor, ObjectDescriptor::File), FxfsError::Inconsistent);
                if id == old_id {
                    break transaction;
                }
                lock_keys = lock_keys![
                    LockKey::object(store.store_object_id(), profile_dir.object_id()),
                    LockKey::object(store.store_object_id(), id)
                ];
                old_id = id;
            } else {
                old_id = INVALID_OBJECT_ID;
                break transaction;
            }
        };

        store.remove_from_graveyard(&mut transaction, self.handle.object_id());
        directory::replace_child_with_object(
            &mut transaction,
            Some((self.handle.object_id(), ObjectDescriptor::File)),
            (&profile_dir, &self.name),
            0,
            false,
            Timestamp::now(),
        )
        .await?;
        transaction.commit().await?;

        if old_id != INVALID_OBJECT_ID {
            fs.graveyard().queue_tombstone_object(store.store_object_id(), old_id);
        }

        Ok(())
    }
}

#[async_trait]
impl RecordingHandle for FileRecordingHandle {
    async fn append<'a>(&'a self, buf: BufferRef<'a>) -> Result<u64, Error> {
        self.handle.write_or_append(None, buf).await.map_err(Into::into)
    }

    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.handle.allocate_buffer(size)
    }

    fn block_size(&self) -> usize {
        self.handle.block_size().get() as usize
    }

    async fn commit(self: Box<Self>) -> Result<(), Error> {
        let store = self.volume.store();
        self.commit_impl().await.inspect_err(|_| {
            store
                .filesystem()
                .graveyard()
                .queue_tombstone_object(store.store_object_id(), self.handle.object_id());
        })
    }

    fn abort_cleanup(self: Box<Self>) {
        self.volume
            .store()
            .filesystem()
            .graveyard()
            .queue_tombstone_object(self.volume.store().store_object_id(), self.handle.object_id());
    }

    fn filter_events(&self) -> bool {
        true
    }
}

/// A handle for storing the profile as an attribute to associate it with an object and tie it to
/// that object's lifetime.
pub struct AttributeRecordingHandle {
    volume: Arc<FxVolume>,
    handle: DataObjectHandle<FxVolume>,
}

impl AttributeRecordingHandle {
    /// Create the profile handle to be stored as an attribute on the object to keep thehe profile
    /// associated with the object and tie the profile lifetime to the object as well.
    pub async fn new(object_id: u64, volume: Arc<FxVolume>) -> Result<Self, Error> {
        let store = volume.store();

        let mut transaction = store.new_transaction(lock_keys![], Options::default()).await?;
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::attribute(
                    object_id,
                    AttributeId::PROFILE_RECORDING,
                    AttributeKey::Attribute,
                ),
                ObjectValue::attribute(0, false),
            ),
        );
        transaction.add(
            store.store_object_id(),
            Mutation::replace_or_insert_object(
                ObjectKey::graveyard_attribute_entry(
                    store.graveyard_directory_object_id(),
                    object_id,
                    AttributeId::PROFILE_RECORDING,
                ),
                ObjectValue::Some,
            ),
        );
        transaction.commit().await?;

        let handle = DataObjectHandle::new(
            volume.clone(),
            object_id,
            false,
            AttributeId::PROFILE_RECORDING,
            0,
            HandleOptions::default(),
            false,
            &[],
        );

        Ok(Self { volume, handle })
    }

    pub async fn commit_impl(&self) -> Result<(), Error> {
        let store = self.volume.store();
        let mut transaction = store.new_transaction(lock_keys![], Options::default()).await?;

        store.remove_attribute_from_graveyard(
            &mut transaction,
            self.handle.object_id(),
            AttributeId::PROFILE_RECORDING,
        );
        transaction.commit().await?;
        Ok(())
    }
}

#[async_trait]
impl RecordingHandle for AttributeRecordingHandle {
    async fn append<'a>(
        &'a self,
        buf: storage_device::buffer::BufferRef<'a>,
    ) -> Result<u64, Error> {
        self.handle.write_or_append(None, buf).await.map_err(Into::into)
    }

    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.handle.allocate_buffer(size)
    }

    fn block_size(&self) -> usize {
        self.handle.block_size().get() as usize
    }

    async fn commit(self: Box<Self>) -> Result<(), Error> {
        self.commit_impl().await.inspect_err(|_| {
            self.abort_cleanup();
        })
    }

    fn abort_cleanup(self: Box<Self>) {
        self.volume.store().filesystem().graveyard().queue_tombstone_attribute(
            self.volume.store().store_object_id(),
            self.handle.object_id(),
            AttributeId::PROFILE_RECORDING,
        );
    }

    fn filter_events(&self) -> bool {
        false
    }
}

trait RecordedVolume: Send + Sync + Sized + Unpin {
    type IdType: std::fmt::Display + Ord + Send + Sized;
    type NodeType: PagerBacked;
    type MessageType: Message<IdType = Self::IdType>;

    fn new(volume: Arc<FxVolume>) -> Self;

    fn open(
        &self,
        id: Self::IdType,
    ) -> impl std::future::Future<Output = Result<OpenedNode<Self::NodeType>, Error>> + Send;

    /// Filters out open markers for files that may not be usable in the profile.
    fn file_is_replayable(
        &self,
        id: &Self::IdType,
    ) -> impl std::future::Future<Output = bool> + Send;

    fn read_and_queue(
        &self,
        handle: Box<dyn ReadObjectHandle>,
        sender: &async_channel::Sender<Request<Self::NodeType>>,
        local_cache: &mut BTreeMap<Self::IdType, Option<OpenedNode<Self::NodeType>>>,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut io_buf = handle.allocate_buffer(IO_SIZE).await;
            let block_size = handle.block_size().get() as usize;
            let file_size = handle.get_size() as usize;
            let mut offset = 0;
            while offset < file_size {
                let actual = handle
                    .read(offset as u64, io_buf.as_mut())
                    .await
                    .map_err(|e| e.context(format!("Failed to read at offset: {}", offset)))?;
                offset += actual;
                let mut local_offset = 0;
                let mut next_block = block_size;
                let mut next_offset = size_of::<Self::MessageType>();
                while next_offset <= actual {
                    let data = io_buf.subslice(local_offset..next_offset);
                    let msg = Self::MessageType::decode_from(data.as_ptr_slice());

                    local_offset = next_offset;
                    next_offset = local_offset + size_of::<Self::MessageType>();
                    // Messages don't overlap block boundaries.
                    if next_offset > next_block {
                        local_offset = next_block;
                        next_offset = local_offset + size_of::<Self::MessageType>();
                        next_block += block_size;
                    }

                    // Ignore trailing zeroes. This is technically a valid entry but extremely
                    // unlikely and will only break an optimization.
                    if msg.is_zeroes() {
                        break;
                    }

                    let file = match local_cache.entry(msg.id()) {
                        Entry::Occupied(entry) => match entry.get() {
                            Some(opened_file) => (*opened_file).clone(),
                            // Found a cached error.
                            None => continue,
                        },
                        Entry::Vacant(entry) => match self.open(msg.id()).await {
                            Err(e) => {
                                debug!("Failed to open object {} from profile: {:?}", msg.id(), e);
                                // Cache the error.
                                entry.insert(None);
                                continue;
                            }
                            Ok(opened_file) => {
                                let file_clone = opened_file.clone();
                                entry.insert(Some(opened_file));
                                file_clone
                            }
                        },
                    };

                    sender.send(Request { file, offset: msg.offset() }).await?;
                }
            }
            Ok(())
        }
    }

    fn record(
        &self,
        recording_handle: Box<dyn RecordingHandle>,
        receiver: async_channel::Receiver<Vec<Self::MessageType>>,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        // Ensure that this gets cleaned up if we cancel or fail anywhere.
        let recording_handle = scopeguard::guard(recording_handle, |recording_handle| {
            recording_handle.abort_cleanup();
        });

        async move {
            let mut recorded_offsets = LinkedHashMap::<Self::MessageType, ()>::new();
            let mut recorded_opens = BTreeMap::<Self::IdType, bool>::new();
            let filter = recording_handle.filter_events();
            while let Ok(buffer) = receiver.recv().await {
                for message in buffer {
                    if message.is_open_marker() {
                        if filter {
                            if let Entry::Vacant(entry) = recorded_opens.entry(message.id()) {
                                let usable = self.file_is_replayable(entry.key()).await;
                                entry.insert(usable);
                            }
                        }
                    } else {
                        recorded_offsets.insert(message, ());
                    }
                }
            }

            let block_size = recording_handle.block_size();
            let mut offset = 0;
            let mut io_buf = recording_handle.allocate_buffer(IO_SIZE).await;
            let mut next_block = block_size;
            while let Some((message, _)) = recorded_offsets.pop_front() {
                // If a file opening was never recorded, or it is not usable drop the message.
                if filter && !recorded_opens.get(&message.id()).copied().unwrap_or(false) {
                    continue;
                }

                let mut next_offset = offset + size_of::<Self::MessageType>();
                if next_offset > next_block {
                    // Zero the remainder of the block. Stopping on block boundaries allows us to
                    // resize the I/O without supporting reading/writing half messages to a buffer.
                    io_buf.as_mut().subslice_mut(offset..next_block).fill(0);
                    if next_block >= IO_SIZE {
                        recording_handle
                            .append(io_buf.as_ref())
                            .await
                            .context("Failed to write profile block")?;
                        offset = 0;
                        next_offset = size_of::<Self::MessageType>();
                        next_block = block_size;
                    } else {
                        offset = next_block;
                        next_offset = offset + size_of::<Self::MessageType>();
                        next_block += block_size;
                    }
                }
                message.encode_to(
                    io_buf.as_mut().subslice_mut(offset..next_offset).as_mut_ptr_slice(),
                );
                offset = next_offset;
            }
            if offset > 0 {
                io_buf.as_mut().subslice_mut(offset..next_block).fill(0);
                recording_handle
                    .append(io_buf.subslice(0..next_block))
                    .await
                    .context("Failed to write profile block")?;
            }

            std::mem::drop(io_buf);
            // Defuse the cleanup.
            let recording_handle = ScopeGuard::into_inner(recording_handle);
            recording_handle.commit().await?;

            Ok(())
        }
    }
}

struct BlobVolume {
    volume: Arc<FxVolume>,
    // Cache the open blob directory here. The Mutex is just to make this Send, but it is not
    // actually used concurrently.
    root_dir: Mutex<Option<Arc<BlobDirectory>>>,
}

impl RecordedVolume for BlobVolume {
    type IdType = Hash;
    type NodeType = FxBlob;
    type MessageType = BlobMessage;

    fn new(volume: Arc<FxVolume>) -> Self {
        Self { volume, root_dir: Mutex::new(None) }
    }

    async fn open(&self, id: Self::IdType) -> Result<OpenedNode<Self::NodeType>, Error> {
        let mut root_dir = self.root_dir.lock().await;
        if root_dir.is_none() {
            *root_dir = Some(
                self.volume
                    .get_or_load_node(
                        self.volume.store().root_directory_object_id(),
                        ObjectDescriptor::Directory,
                        None,
                    )
                    .await?
                    .into_any()
                    .downcast::<BlobDirectory>()
                    .map_err(|_| FxfsError::Inconsistent)?,
            );
        };
        root_dir
            .as_ref()
            .unwrap()
            .open_blob(&id.into())
            .await?
            .ok_or_else(|| FxfsError::NotFound.into())
    }

    async fn file_is_replayable(&self, _id: &Self::IdType) -> bool {
        // There is nothing is filter out in blob volumes.
        true
    }
}

struct FileVolume {
    volume: Arc<FxVolume>,
}

impl RecordedVolume for FileVolume {
    type IdType = u64;
    type NodeType = FxFile;
    type MessageType = FileMessage;

    fn new(volume: Arc<FxVolume>) -> Self {
        Self { volume }
    }

    async fn open(&self, id: Self::IdType) -> Result<OpenedNode<Self::NodeType>, Error> {
        self.volume
            .get_or_load_node(id, ObjectDescriptor::File, None)
            .await?
            .into_any()
            .downcast::<FxFile>()
            .map_err(|_| anyhow!("Non-file opened"))?
            .into_opened_node()
            .ok_or_else(|| anyhow!("File being purged"))
    }

    async fn file_is_replayable(&self, id: &Self::IdType) -> bool {
        match self.volume.store().get_keys(*id).await {
            // If any keys are not the volume key id, then the file may not be readable later.
            // If there's more than one, then at least one is not the volume key.
            Ok(keys)
                if keys.is_empty()
                    || (keys.len() == 1 && keys.first().unwrap().0 == VOLUME_DATA_KEY_ID) =>
            {
                true
            }
            _ => false,
        }
    }
}

trait Message: Eq + PartialEq + Sized + Send + Sync + std::hash::Hash + 'static {
    type IdType: std::fmt::Display + Ord + Send + Sized;

    fn id(&self) -> Self::IdType;
    fn offset(&self) -> u64;
    fn encode_to(&self, dest: MutPtrByteSlice<'_>);
    fn decode_from(src: PtrByteSlice<'_>) -> Self;
    fn is_zeroes(&self) -> bool;
    fn from_node_request(node: Arc<dyn FxNode>, offset: u64) -> Result<Self, Error>;
    fn is_open_marker(&self) -> bool;
}

#[derive(Debug, Eq, std::hash::Hash, PartialEq)]
struct BlobMessage {
    id: Hash,
    // Don't bother with offset+length. The kernel is going split up and align it one way and then
    // we're going to change it all with read-ahead/read-around.
    offset: u64,
}

impl BlobMessage {
    fn encode_to_impl(&self, dest: &mut [u8; size_of::<Self>()]) {
        let (first, second) = mut_array_refs![dest, size_of::<Hash>(), size_of::<u64>()];
        *first = self.id.into();
        *second = self.offset.to_le_bytes();
    }

    fn decode_from_impl(src: &[u8; size_of::<Self>()]) -> Self {
        let (first, second) = array_refs!(src, size_of::<Hash>(), size_of::<u64>());
        Self { id: Hash::from_array(*first), offset: u64::from_le_bytes(*second) }
    }
}

impl Message for BlobMessage {
    type IdType = Hash;

    fn id(&self) -> Self::IdType {
        self.id
    }

    fn offset(&self) -> u64 {
        self.offset
    }

    fn encode_to(&self, mut dest: MutPtrByteSlice<'_>) {
        let mut buf = [0u8; size_of::<Self>()];
        self.encode_to_impl(&mut buf);
        dest.copy_from_slice(&buf);
    }

    fn decode_from(src: PtrByteSlice<'_>) -> Self {
        let mut buf = [0u8; size_of::<Self>()];
        src.copy_to_slice(&mut buf);
        Self::decode_from_impl(&buf)
    }

    fn is_zeroes(&self) -> bool {
        self.id == Hash::from_array([0u8; size_of::<Hash>()]) && self.offset == 0
    }

    fn from_node_request(node: Arc<dyn FxNode>, offset: u64) -> Result<Self, Error> {
        match node.into_any().downcast::<FxBlob>() {
            Ok(blob) => Ok(Self { id: blob.root(), offset }),
            Err(_) => Err(anyhow!("Cannot record non-blob entry.")),
        }
    }

    fn is_open_marker(&self) -> bool {
        self.offset == FILE_OPEN_MARKER
    }
}

#[derive(Debug, Eq, std::hash::Hash, PartialEq)]
struct FileMessage {
    id: u64,
    // Don't bother with offset+length. The kernel is going split up and align it one way and then
    // we're going to change it all with read-ahead/read-around.
    offset: u64,
}

impl FileMessage {
    fn encode_to_impl(&self, dest: &mut [u8; size_of::<Self>()]) {
        let (first, second) = mut_array_refs![dest, size_of::<u64>(), size_of::<u64>()];
        *first = self.id.to_le_bytes();
        *second = self.offset.to_le_bytes();
    }

    fn decode_from_impl(src: &[u8; size_of::<Self>()]) -> Self {
        let (first, second) = array_refs!(src, size_of::<u64>(), size_of::<u64>());
        Self { id: u64::from_le_bytes(*first), offset: u64::from_le_bytes(*second) }
    }
}

impl Message for FileMessage {
    type IdType = u64;

    fn id(&self) -> Self::IdType {
        self.id
    }

    fn offset(&self) -> u64 {
        self.offset
    }

    fn encode_to(&self, mut dest: MutPtrByteSlice<'_>) {
        let mut buf = [0u8; size_of::<Self>()];
        self.encode_to_impl(&mut buf);
        dest.copy_from_slice(&buf);
    }

    fn decode_from(src: PtrByteSlice<'_>) -> Self {
        let mut buf = [0u8; size_of::<Self>()];
        src.copy_to_slice(&mut buf);
        Self::decode_from_impl(&buf)
    }

    fn is_zeroes(&self) -> bool {
        self.id == 0 && self.offset == 0
    }

    fn from_node_request(node: Arc<dyn FxNode>, offset: u64) -> Result<Self, Error> {
        match node.into_any().downcast::<FxFile>() {
            Ok(file) => Ok(Self { id: file.object_id(), offset }),
            Err(_) => Err(anyhow!("Cannot record non-file entry")),
        }
    }

    fn is_open_marker(&self) -> bool {
        self.offset == FILE_OPEN_MARKER
    }
}

/// Takes messages to be written into the current profile. This should be dropped before the
/// recording is stopped to ensure that all messages have been flushed to the writer thread.
pub trait Recorder: Send + Sync {
    /// Record a page in request, for the given identifier and offset.
    fn record(&mut self, node: Arc<dyn FxNode>, offset: u64) -> Result<(), Error>;

    /// Record file opens to gather what files were actually used during the recording.
    fn record_open(&mut self, node: Arc<dyn FxNode>) -> Result<(), Error>;
}

struct RecorderImpl<T: Message> {
    sender: async_channel::Sender<Vec<T>>,
    buffer: Vec<T>,
}

impl<T: Message> RecorderImpl<T> {
    fn new(sender: async_channel::Sender<Vec<T>>) -> Self {
        Self { sender, buffer: Vec::with_capacity(MESSAGE_CHUNK_SIZE) }
    }
}

impl<T: Message> Recorder for RecorderImpl<T> {
    fn record(&mut self, node: Arc<dyn FxNode>, offset: u64) -> Result<(), Error> {
        self.buffer.push(T::from_node_request(node, offset)?);
        if self.buffer.len() >= MESSAGE_CHUNK_SIZE {
            // try_send to avoid async await, we use an unbounded channel anyways so any failure
            // here should only be if the channel is closed, which is permanent anyways.
            self.sender.try_send(std::mem::replace(
                &mut self.buffer,
                Vec::with_capacity(MESSAGE_CHUNK_SIZE),
            ))?;
        }
        RECORDED.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn record_open(&mut self, node: Arc<dyn FxNode>) -> Result<(), Error> {
        self.record(node, FILE_OPEN_MARKER)
    }
}

impl<T: Message> Drop for RecorderImpl<T> {
    fn drop(&mut self) {
        // Best effort sending what messages have already been queued.
        if self.buffer.len() > 0 {
            let buffer = std::mem::take(&mut self.buffer);
            let _ = self.sender.try_send(buffer);
        }
    }
}

struct Request<P: PagerBacked> {
    file: Arc<P>,
    offset: u64,
}

struct ReplayState<T> {
    replay_threads: future::Shared<BoxFuture<'static, ()>>,
    _cache_task: fasync::Task<()>,
    _phantom: PhantomData<T>,
}

impl<T: RecordedVolume> ReplayState<T> {
    fn new(handle: Box<dyn ReadObjectHandle>, volume: Arc<FxVolume>, guard: ActiveGuard) -> Self {
        let (sender, receiver) = async_channel::unbounded::<Request<T::NodeType>>();

        // Create async_channel. An async thread reads and populates the channel, then N threads
        // consume it and touch pages.
        let mut replay_threads = Vec::with_capacity(REPLAY_THREADS);
        for _ in 0..REPLAY_THREADS {
            let receiver = receiver.clone();
            // The replay threads can have references to files so we make sure they have a guard
            // so that shutdown will wait till they have been joined.
            let guard = guard.clone();
            replay_threads.push(fasync::unblock(move || {
                let _guard = guard;
                Self::page_in_thread(receiver);
            }));
        }
        let replay_threads = (Box::pin(async {
            join_all(replay_threads).await;
        }) as BoxFuture<'static, ()>)
            .shared();

        let scope = volume.scope().clone();
        let cache_task = scope
            .spawn({
                // The replay threads hold active guards, so we must watch for cancellation.  When
                // cancelled, we'll drop the sender which will cause the replay threads to drop
                // their guards, which will allow shutdown to proceed.
                async move {
                    let mut task = pin!(
                        async {
                            // Hold the items in cache until replay is stopped. Optional as None
                            // indicates that the file could not be opened, and we want to cache that
                            // failure.
                            let mut local_cache: BTreeMap<
                                T::IdType,
                                Option<OpenedNode<T::NodeType>>,
                            > = BTreeMap::new();

                            let volume_id = volume.id();

                            if let Err(error) = T::new(volume)
                                .read_and_queue(handle, &sender, &mut local_cache)
                                .await
                            {
                                error!(error:?; "Failed to read back profile");
                            }
                            sender.close();

                            info!(
                                "Replay for volume {} opened {} of {} objects.",
                                volume_id,
                                local_cache.iter().filter(|(_, e)| e.is_some()).count(),
                                local_cache.len()
                            );

                            // Keep the cache alive until dropped.
                            let () = std::future::pending().await;
                        }
                        .fuse()
                    );

                    select! {
                        _ = task => {}
                        _ = guard.on_cancel().fuse() => {}
                    }
                }
            })
            .into();

        Self { replay_threads, _cache_task: cache_task, _phantom: PhantomData }
    }

    fn page_in_thread(queue: async_channel::Receiver<Request<T::NodeType>>) {
        while let Ok(request) = queue.recv_blocking() {
            let res =
                request.file.vmo().op_range(zx::VmoOp::PREFETCH, request.offset, PAGE_SIZE.get());
            if let Err(e) = res {
                warn!("Failed to prefetch page: {:?}", e);
            }
            // If the volume is shutdown, the sender will be dropped.
            if queue.sender_count() == 0 {
                return;
            }
        }
    }
}

/// Holds the current profile recording and/or replay state, and provides methods for state
/// transitions.
#[async_trait]
pub trait ProfileState: Send + Sync {
    /// Creates a new recording and returns the `Recorder` object to record to. The recording
    /// finalizes when the associated `Recorder` is dropped.  Stops any recording currently in
    /// progress.
    fn record_new(
        &mut self,
        volume: &Arc<FxVolume>,
        recording_handle: Box<dyn RecordingHandle>,
    ) -> Box<dyn Recorder>;

    /// Reads given handle to parse a profile and replay it by requesting pages via
    /// ZX_VMO_OP_PREFETCH in blocking background threads. Stops any replay currently in progress.
    fn replay_profile(
        &mut self,
        handle: Box<dyn ReadObjectHandle>,
        volume: Arc<FxVolume>,
        guard: ActiveGuard,
    );

    /// Waits for replay to finish, but does not drop the cache.  The cache will be dropped when
    /// the ProfileState impl is dropped.  This is fine to call multiple times.
    async fn wait_for_replay_to_finish(&mut self);

    /// Waits for the recording to finish.
    async fn wait_for_recording_to_finish(&mut self);
}

pub fn new_profile_state(is_blob: bool) -> Box<dyn ProfileState> {
    if is_blob {
        Box::new(ProfileStateImpl::<BlobVolume>::new())
    } else {
        Box::new(ProfileStateImpl::<FileVolume>::new())
    }
}

struct ProfileStateImpl<T> {
    recording: Option<fasync::CancelableJoinHandle<()>>,
    replay: Option<ReplayState<T>>,
}

impl<T> ProfileStateImpl<T> {
    fn new() -> Self {
        Self { recording: None, replay: None }
    }
}

#[async_trait]
impl<T: RecordedVolume> ProfileState for ProfileStateImpl<T> {
    fn record_new(
        &mut self,
        volume: &Arc<FxVolume>,
        recording_handle: Box<dyn RecordingHandle>,
    ) -> Box<dyn Recorder> {
        let (sender, receiver) = async_channel::unbounded();
        let volume = volume.clone();
        // Cancel the previous recording (if any).
        self.recording = None;
        let scope = volume.scope().clone();
        self.recording = Some(
            scope
                .spawn(async move {
                    let recording = T::new(volume);
                    if let Err(error) = recording.record(recording_handle, receiver).await {
                        warn!(error:?; "Profile recording failed");
                    }
                })
                .into(),
        );
        Box::new(RecorderImpl::new(sender))
    }

    fn replay_profile(
        &mut self,
        handle: Box<dyn ReadObjectHandle>,
        volume: Arc<FxVolume>,
        guard: ActiveGuard,
    ) {
        self.replay = Some(ReplayState::new(handle, volume, guard));
    }

    async fn wait_for_replay_to_finish(&mut self) {
        if let Some(replay) = &mut self.replay {
            replay.replay_threads.clone().await;
        }
    }

    async fn wait_for_recording_to_finish(&mut self) {
        if let Some(recording) = self.recording.take() {
            let _ = recording.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AttributeRecordingHandle, BlobMessage, BlobVolume, FileMessage, FileRecordingHandle,
        FileVolume, IO_SIZE, Message, MutPtrByteSlice, PtrByteSlice, RecordedVolume, Request,
        new_profile_state,
    };
    use crate::fuchsia::file::FxFile;
    use crate::fuchsia::fxblob::blob::FxBlob;
    use crate::fuchsia::fxblob::testing::{BlobFixture, new_blob_fixture, open_blob_fixture};
    use crate::fuchsia::node::{FxNode, OpenedNode};
    use crate::fuchsia::pager::PagerBacked;
    use crate::fuchsia::testing::{TestFixture, TestFixtureOptions, open_file_checked};
    use crate::fuchsia::volume::FxVolume;
    use anyhow::Error;
    use async_trait::async_trait;
    use delivery_blob::CompressionMode;
    use event_listener::{Event, EventListener};
    use fidl_fuchsia_fs_startup::CreateOptions;
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use fuchsia_hash::Hash;
    use fuchsia_sync::Mutex;
    use fxfs::object_handle::{ObjectHandle, ReadObjectHandle, WriteObjectHandle};
    use fxfs::object_store::object_record::ObjectItem;
    use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
    use fxfs::object_store::{
        AttributeId, AttributeKey, DataObjectHandle, HandleOptions, ObjectDescriptor, ObjectKey,
        ObjectStore, ObjectValue,
    };
    use std::collections::BTreeMap;
    use std::mem::size_of;
    use std::sync::Arc;
    use std::time::Duration;
    use storage_device::buffer::{BufferRef, MutableBufferRef};
    use storage_device::buffer_allocator::{BufferAllocator, BufferFuture, BufferSource};
    use storage_units::BlockSize;

    struct FakeReaderWriterInner {
        data: Vec<u8>,
        delays: Vec<EventListener>,
    }

    struct FakeReaderWriter {
        allocator: BufferAllocator,
        inner: Arc<Mutex<FakeReaderWriterInner>>,
    }

    const BLOCK_SIZE: usize = 4096;

    impl FakeReaderWriter {
        fn new() -> Self {
            Self {
                allocator: BufferAllocator::new(BLOCK_SIZE, BufferSource::new(IO_SIZE * 2)),
                inner: Arc::new(Mutex::new(FakeReaderWriterInner {
                    data: Vec::new(),
                    delays: Vec::new(),
                })),
            }
        }

        fn push_delay(&self, delay: EventListener) {
            self.inner.lock().delays.insert(0, delay);
        }
    }

    impl ObjectHandle for FakeReaderWriter {
        fn object_id(&self) -> u64 {
            0
        }

        fn block_size(&self) -> BlockSize {
            BlockSize::new(self.allocator.block_size() as u32).unwrap()
        }

        fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
            self.allocator.allocate_buffer(size)
        }
    }

    impl WriteObjectHandle for FakeReaderWriter {
        async fn write_or_append(
            &self,
            offset: Option<u64>,
            buf: BufferRef<'_>,
        ) -> Result<u64, Error> {
            // We only append for now.
            assert!(offset.is_none());
            let delay = self.inner.lock().delays.pop();
            if let Some(delay) = delay {
                delay.await;
            }
            // This relocking has a TOCTOU flavour, but it shouldn't matter for this application.
            buf.append_to(&mut self.inner.lock().data);
            Ok(buf.len() as u64)
        }

        async fn truncate(&self, _size: u64) -> Result<(), Error> {
            unreachable!();
        }

        async fn flush(&self) -> Result<(), Error> {
            unreachable!();
        }
    }

    async fn write_file(fixture: &TestFixture, name: &str, data: &[u8]) -> u64 {
        let root_dir = fixture.volume().root_dir();
        let mut transaction = fixture
            .volume()
            .volume()
            .store()
            .new_transaction(
                lock_keys![LockKey::object(
                    fixture.volume().volume().store().store_object_id(),
                    root_dir.object_id()
                )],
                Options::default(),
            )
            .await
            .expect("Creating transaction for new file");
        let id = root_dir
            .directory()
            .create_child_file(&mut transaction, name)
            .await
            .expect("Creating new_file")
            .object_id();
        transaction.commit().await.unwrap();
        let file = open_file_checked(
            fixture.root(),
            name,
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        file.write(data).await.unwrap().expect("Writing file");
        id
    }

    #[async_trait]
    impl ReadObjectHandle for FakeReaderWriter {
        async fn read(&self, offset: u64, buf: MutableBufferRef<'_>) -> Result<usize, Error> {
            let delay = self.inner.lock().delays.pop();
            if let Some(delay) = delay {
                delay.await;
            }
            // This relocking has a TOCTOU flavour, but it shouldn't matter for this application.
            let inner = self.inner.lock();
            assert!(offset as usize <= inner.data.len());
            let offset_end = std::cmp::min(offset as usize + buf.len(), inner.data.len());
            let size = offset_end - offset as usize;
            buf.subslice_mut(..size).copy_from_slice(&inner.data[offset as usize..offset_end]);
            Ok(size)
        }

        fn get_size(&self) -> u64 {
            self.inner.lock().data.len() as u64
        }
    }

    #[fuchsia::test]
    async fn test_encode_decode_blob() {
        let mut buf = [0u8; size_of::<BlobMessage>()];
        let m = BlobMessage { id: [88u8; 32].into(), offset: 77 };
        m.encode_to(MutPtrByteSlice::from(&mut buf[..]));
        let m2 = BlobMessage::decode_from(PtrByteSlice::from(&buf[..]));
        assert_eq!(m, m2);
    }

    #[fuchsia::test]
    async fn test_encode_decode_file() {
        let mut buf = [0u8; size_of::<FileMessage>()];
        let m = FileMessage { id: 88, offset: 77 };
        m.encode_to(MutPtrByteSlice::from(&mut buf[..]));
        let m2 = FileMessage::decode_from(PtrByteSlice::from(&buf[..]));
        assert!(!m2.is_zeroes());
        assert_eq!(m, m2);
    }

    const TEST_PROFILE_NAME: &str = "test_profile";

    async fn get_test_profile_handle(volume: &Arc<FxVolume>) -> DataObjectHandle<FxVolume> {
        let profile_dir = volume.get_profile_directory().await.unwrap();
        ObjectStore::open_object(
            volume,
            profile_dir
                .lookup(TEST_PROFILE_NAME)
                .await
                .expect("lookup failed")
                .expect("not found")
                .0,
            HandleOptions::default(),
            None,
        )
        .await
        .unwrap()
    }

    async fn get_test_profile_contents(volume: &Arc<FxVolume>) -> Vec<u8> {
        get_test_profile_handle(volume).await.contents(1024 * 1024).await.unwrap().to_vec()
    }

    #[fuchsia::test]
    async fn test_recording_basic_blob() {
        let fixture = new_blob_fixture().await;
        {
            let hash = fixture.write_blob(&[88u8], CompressionMode::Never).await;
            let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");

            let mut state = new_profile_state(true);
            let volume = fixture.volume().volume();

            {
                // Drop recorder when finished writing to flush data.
                let handle =
                    FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
                let mut recorder = state.record_new(volume, Box::new(handle));
                recorder.record(blob.clone(), 0).unwrap();
                recorder.record_open(blob).unwrap();
            }

            state.wait_for_recording_to_finish().await;

            assert_eq!(get_test_profile_contents(volume).await.len(), BLOCK_SIZE);
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_recording_basic_file() {
        let fixture = TestFixture::new().await;
        {
            let id = write_file(&fixture, "foo", &[88u8]).await;
            let node = fixture
                .volume()
                .volume()
                .get_or_load_node(id, ObjectDescriptor::File, Some(fixture.volume().root_dir()))
                .await
                .unwrap();

            let mut state = new_profile_state(false);
            let volume = fixture.volume().volume();

            {
                // Drop recorder when finished writing to flush data.
                let handle =
                    FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
                let mut recorder = state.record_new(volume, Box::new(handle));
                recorder.record(node.clone(), 0).unwrap();
                recorder.record_open(node).unwrap();
            }
            state.wait_for_recording_to_finish().await;

            assert_eq!(get_test_profile_contents(volume).await.len(), BLOCK_SIZE);
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_recording_filtered_without_open() {
        let fixture = new_blob_fixture().await;
        {
            let hash = fixture.write_blob(&[88u8], CompressionMode::Never).await;
            let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");

            let mut state = new_profile_state(true);
            let volume = fixture.volume().volume();

            {
                // Drop recorder when finished writing to flush data.
                let handle =
                    FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
                let mut recorder = state.record_new(volume, Box::new(handle));
                recorder.record(blob.clone(), 0).unwrap();
            }
            state.wait_for_recording_to_finish().await;

            assert_eq!(get_test_profile_contents(volume).await.len(), 0);
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_associated_recording_not_filtered_without_open() {
        let fixture = new_blob_fixture().await;
        {
            let hash = fixture.write_blob(&[88u8], CompressionMode::Never).await;
            let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");

            let mut state = new_profile_state(true);
            let volume = fixture.volume().volume();

            {
                // Drop recorder when finished writing to flush data.
                let handle =
                    AttributeRecordingHandle::new(blob.object_id(), volume.clone()).await.unwrap();
                let mut recorder = state.record_new(volume, Box::new(handle));
                recorder.record(blob.clone(), 0).unwrap();
            }
            state.wait_for_recording_to_finish().await;

            // The profile is stored as an attribute on the blob itself.
            // Let's get the attribute contents to see if it recorded the page.
            let key = ObjectKey::attribute(
                blob.object_id(),
                AttributeId::PROFILE_RECORDING,
                AttributeKey::Attribute,
            );
            let store = volume.store();
            let ObjectItem { value, .. } =
                store.tree().find(&key).await.unwrap().expect("profile attribute not found");
            let size = match value {
                ObjectValue::Attribute { size, .. } => size,
                _ => panic!("Expected Attribute value"),
            };

            assert_eq!(size, BLOCK_SIZE as u64);
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_recording_blob_more_than_block() {
        let mut state = new_profile_state(true);

        let fixture = new_blob_fixture().await;
        assert_eq!(BLOCK_SIZE as u64, fixture.fs().block_size());
        let message_count =
            (fixture.fs().block_size().get() as usize / size_of::<BlobMessage>()) + 1;
        let hash;
        let volume = fixture.volume().volume();

        {
            hash = fixture.write_blob(&[88u8], CompressionMode::Never).await;
            let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");
            // Drop recorder when finished writing to flush data.
            let handle = FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
            let mut recorder = state.record_new(volume, Box::new(handle));
            recorder.record_open(blob.clone()).unwrap();
            for i in 0..message_count {
                recorder.record(blob.clone(), 4096 * i as u64).unwrap();
            }
        }
        state.wait_for_recording_to_finish().await;

        assert_eq!(get_test_profile_contents(volume).await.len(), BLOCK_SIZE * 2);

        let mut local_cache: BTreeMap<Hash, Option<OpenedNode<FxBlob>>> = BTreeMap::new();
        let (sender, receiver) = async_channel::unbounded::<Request<FxBlob>>();

        let volume = fixture.volume().volume().clone();
        let task = fasync::Task::spawn(async move {
            let handle = Box::new(get_test_profile_handle(&volume).await);
            let blob = BlobVolume::new(volume);
            blob.read_and_queue(handle, &sender, &mut local_cache).await.unwrap();
        });

        let mut recv_count = 0;
        while let Ok(msg) = receiver.recv().await {
            assert_eq!(msg.file.root(), hash);
            assert_eq!(msg.offset, 4096 * recv_count);
            recv_count += 1;
        }
        task.await;
        assert_eq!(recv_count, message_count as u64);

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_recording_file_more_than_block() {
        let mut state = new_profile_state(false);

        let fixture = TestFixture::new().await;
        assert_eq!(BLOCK_SIZE as u64, fixture.fs().block_size());
        let message_count =
            (fixture.fs().block_size().get() as usize / size_of::<FileMessage>()) + 1;
        let id;
        let volume = fixture.volume().volume();
        {
            id = write_file(&fixture, "foo", &[88u8]).await;
            let node = volume
                .get_or_load_node(id, ObjectDescriptor::File, Some(fixture.volume().root_dir()))
                .await
                .unwrap();
            // Drop recorder when finished writing to flush data.
            let handle = FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
            let mut recorder = state.record_new(volume, Box::new(handle));
            recorder.record_open(node.clone()).unwrap();
            for i in 0..message_count {
                recorder.record(node.clone(), 4096 * i as u64).unwrap();
            }
        }
        state.wait_for_recording_to_finish().await;

        assert_eq!(get_test_profile_contents(volume).await.len(), BLOCK_SIZE * 2);

        let mut local_cache: BTreeMap<u64, Option<OpenedNode<FxFile>>> = BTreeMap::new();
        let (sender, receiver) = async_channel::unbounded::<Request<FxFile>>();

        let volume = fixture.volume().volume().clone();
        let task = fasync::Task::spawn(async move {
            let handle = Box::new(get_test_profile_handle(&volume).await);
            let file = FileVolume::new(volume);
            file.read_and_queue(handle, &sender, &mut local_cache).await.unwrap();
        });

        let mut recv_count = 0;
        while let Ok(msg) = receiver.recv().await {
            assert_eq!(msg.file.object_id(), id);
            assert_eq!(msg.offset, 4096 * recv_count);
            recv_count += 1;
        }
        task.await;
        assert_eq!(recv_count, message_count as u64);

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_recording_more_than_io_size() {
        let fixture = new_blob_fixture().await;

        {
            let mut state = new_profile_state(true);
            let message_count = (IO_SIZE as usize / size_of::<BlobMessage>()) + 1;
            let hash;
            let volume = fixture.volume().volume();
            {
                hash = fixture.write_blob(&[88u8], CompressionMode::Never).await;
                let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");
                // Drop recorder when finished writing to flush data.
                let handle =
                    FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
                let mut recorder = state.record_new(volume, Box::new(handle));
                recorder.record_open(blob.clone()).unwrap();
                for i in 0..message_count {
                    recorder.record(blob.clone(), 4096 * i as u64).unwrap();
                }
            }
            state.wait_for_recording_to_finish().await;
            assert_eq!(get_test_profile_contents(volume).await.len(), IO_SIZE + BLOCK_SIZE);

            let mut local_cache: BTreeMap<Hash, Option<OpenedNode<FxBlob>>> = BTreeMap::new();
            let (sender, receiver) = async_channel::unbounded::<Request<FxBlob>>();

            let volume = volume.clone();
            let task = fasync::Task::spawn(async move {
                let handle = Box::new(get_test_profile_handle(&volume).await);
                let blob = BlobVolume::new(volume);
                blob.read_and_queue(handle, &sender, &mut local_cache).await.unwrap();
            });

            let mut recv_count = 0;
            while let Ok(msg) = receiver.recv().await {
                assert_eq!(msg.file.root(), hash);
                assert_eq!(msg.offset, 4096 * recv_count);
                recv_count += 1;
            }
            task.await;
            assert_eq!(recv_count, message_count as u64);
        }

        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_replay_profile_blob() {
        // Create all the files that we need first, then restart the filesystem to clear cache.
        let mut state = new_profile_state(true);

        let mut hashes = Vec::new();

        let fixture = new_blob_fixture().await;
        {
            assert_eq!(BLOCK_SIZE as u64, fixture.fs().block_size());
            let message_count =
                (fixture.fs().block_size().get() as usize / size_of::<BlobMessage>()) + 1;

            let volume = fixture.volume().volume();
            let handle = FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
            let mut recorder = state.record_new(volume, Box::new(handle));
            // Page in the zero offsets only to avoid readahead strangeness.
            for i in 0..message_count {
                let hash =
                    fixture.write_blob(i.to_string().as_bytes(), CompressionMode::Never).await;
                let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");
                recorder.record_open(blob.clone()).unwrap();
                hashes.push(hash);
                recorder.record(blob.clone(), 0).unwrap();
            }
        };
        let device = fixture.close().await;
        device.ensure_unique();
        state.wait_for_recording_to_finish().await;

        device.reopen(false);
        let fixture = open_blob_fixture(device).await;
        {
            // Need to get the root vmo to check committed bytes.
            // Ensure that nothing is paged in right now.
            for hash in &hashes {
                let blob = fixture.get_blob(*hash).await.expect("Opening blob");
                assert_eq!(blob.vmo().info().unwrap().committed_bytes, 0);
            }

            let volume = fixture.volume().volume();
            state.replay_profile(
                Box::new(get_test_profile_handle(volume).await),
                volume.clone(),
                volume.scope().try_active_guard().unwrap(),
            );

            // Await all data being played back by checking that things have paged in.
            for hash in &hashes {
                let blob = fixture.get_blob(*hash).await.expect("Opening blob");
                while blob.vmo().info().unwrap().committed_bytes == 0 {
                    fasync::Timer::new(Duration::from_millis(25)).await;
                }
            }
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_replay_profile_file() {
        // Create all the files that we need first, then restart the filesystem to clear cache.
        let mut state = new_profile_state(false);

        let mut ids = Vec::new();

        let fixture = TestFixture::new().await;
        {
            assert_eq!(BLOCK_SIZE as u64, fixture.fs().block_size());
            let message_count =
                (fixture.fs().block_size().get() as usize / size_of::<FileMessage>()) + 1;

            let volume = fixture.volume().volume();
            let handle = FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
            let mut recorder = state.record_new(volume, Box::new(handle));
            // Page in the zero offsets only to avoid readahead strangeness.
            for i in 0..message_count {
                let id = write_file(&fixture, &i.to_string(), &[88u8]).await;
                let node = fixture
                    .volume()
                    .volume()
                    .get_or_load_node(id, ObjectDescriptor::File, Some(fixture.volume().root_dir()))
                    .await
                    .unwrap();
                recorder.record_open(node.clone()).unwrap();
                ids.push(id);
                recorder.record(node.clone(), 0).unwrap();
            }
        };
        let device = fixture.close().await;
        device.ensure_unique();
        state.wait_for_recording_to_finish().await;

        device.reopen(false);
        let fixture = TestFixture::open(
            device,
            TestFixtureOptions { encrypted: true, format: false, ..Default::default() },
        )
        .await;
        {
            // Ensure that nothing is paged in right now.
            for id in &ids {
                let file = fixture
                    .volume()
                    .volume()
                    .get_or_load_node(
                        *id,
                        ObjectDescriptor::File,
                        Some(fixture.volume().root_dir()),
                    )
                    .await
                    .unwrap()
                    .into_any()
                    .downcast::<FxFile>()
                    .unwrap();
                assert_eq!(file.vmo().info().unwrap().committed_bytes, 0);
            }

            let volume = fixture.volume().volume();
            state.replay_profile(
                Box::new(get_test_profile_handle(volume).await),
                volume.clone(),
                volume.scope().try_active_guard().unwrap(),
            );

            // Await all data being played back by checking that things have paged in.
            for id in &ids {
                let file = fixture
                    .volume()
                    .volume()
                    .get_or_load_node(
                        *id,
                        ObjectDescriptor::File,
                        Some(fixture.volume().root_dir()),
                    )
                    .await
                    .unwrap()
                    .into_any()
                    .downcast::<FxFile>()
                    .unwrap();
                while file.vmo().info().unwrap().committed_bytes == 0 {
                    fasync::Timer::new(Duration::from_millis(25)).await;
                }
            }
            state.wait_for_recording_to_finish().await;
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_recording_during_replay() {
        let mut state = new_profile_state(true);

        let hash;
        let first_recording;
        let fixture = new_blob_fixture().await;
        let volume = fixture.volume().volume();

        // First make a simple recording.
        {
            let handle = FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
            let mut recorder = state.record_new(volume, Box::new(handle));
            hash = fixture.write_blob(&[0, 1, 2, 3], CompressionMode::Never).await;
            let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");
            recorder.record_open(blob.clone()).unwrap();
            recorder.record(blob.clone(), 0).unwrap();
        }

        state.wait_for_recording_to_finish().await;
        first_recording = get_test_profile_contents(volume).await;
        assert_ne!(first_recording.len(), 0);
        let device = fixture.close().await;
        device.ensure_unique();

        device.reopen(false);
        let fixture = open_blob_fixture(device).await;

        {
            // Need to get the root vmo to check committed bytes.
            // Ensure that nothing is paged in right now.
            let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");
            assert_eq!(blob.vmo().info().unwrap().committed_bytes, 0);

            // Start recording
            let volume = fixture.volume().volume();
            let handle = FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
            let mut recorder = state.record_new(volume, Box::new(handle));
            recorder.record(blob.clone(), 4096).unwrap();

            // Replay the original recording.
            let volume = fixture.volume().volume();
            state.replay_profile(
                Box::new(get_test_profile_handle(volume).await),
                volume.clone(),
                volume.scope().try_active_guard().unwrap(),
            );

            // Await all data being played back by checking that things have paged in.
            {
                let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");
                while blob.vmo().info().unwrap().committed_bytes == 0 {
                    fasync::Timer::new(Duration::from_millis(25)).await;
                }
            }

            // Record the open after the replay. Needs both the before and after action to
            // capture anything ensuring that the two procedures overlapped.
            recorder.record_open(blob.clone()).unwrap();
        }

        state.wait_for_recording_to_finish().await;

        let volume = fixture.volume().volume();
        let second_recording = get_test_profile_contents(volume).await;
        assert_ne!(second_recording.len(), 0);
        assert_ne!(&second_recording, &first_recording);

        fixture.close().await;
    }

    // Doesn't ensure that anything reads back properly, just that everything shuts down when
    // stopped early.
    #[fuchsia::test]
    async fn test_replay_profile_stop_reading_early() {
        let mut state = new_profile_state(true);
        let fixture = new_blob_fixture().await;

        {
            let volume = fixture.volume().volume();

            // Create the file that we need first.
            let message;
            {
                let hash = fixture.write_blob(&[0, 1, 2, 3], CompressionMode::Never).await;
                let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");
                message = BlobMessage { id: blob.root(), offset: 0 };
            }
            state.wait_for_recording_to_finish().await;

            // Make a profile long enough to require 2 reads.
            let replay_handle = Box::new(FakeReaderWriter::new());
            let mut buff = vec![0u8; IO_SIZE * 2];
            message.encode_to_impl((&mut buff[0..size_of::<BlobMessage>()]).try_into().unwrap());
            message.encode_to_impl(
                (&mut buff[IO_SIZE..IO_SIZE + size_of::<BlobMessage>()]).try_into().unwrap(),
            );

            replay_handle.inner.lock().data = buff;
            let delay1 = Event::new();
            replay_handle.push_delay(delay1.listen());
            let delay2 = Event::new();
            replay_handle.push_delay(delay2.listen());

            state.replay_profile(
                replay_handle,
                volume.clone(),
                volume.scope().try_active_guard().unwrap(),
            );

            // Delay the first read long enough so that the stop can be triggered during it.
            fasync::Task::spawn(async move {
                // Let the profiler wait on this a little.
                fasync::Timer::new(Duration::from_millis(100)).await;
                delay1.notify(usize::MAX);
            })
            .detach();
        }

        // The reader should block indefinitely (we never notify delay2), but that shouldn't block
        // termination.
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_replay_blob_missing() {
        let fixture = new_blob_fixture().await;
        // Create the blob that comes after the missing blob. Ensure it still gets
        // recorded.
        let hash = fixture.write_blob(&[0, 1, 2, 3], CompressionMode::Never).await;
        let mut buff = vec![0u8; IO_SIZE];
        {
            // First encode the blob that is missing. Just make it up. This will be skipped during
            // replay.
            {
                let message = BlobMessage { id: [42u8; 32].into(), offset: 0 };
                message
                    .encode_to_impl((&mut buff[0..size_of::<BlobMessage>()]).try_into().unwrap());
            }

            // Create the blob that won't be missing and encode that.
            {
                let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");
                let message = BlobMessage { id: blob.root(), offset: 0 };
                message.encode_to_impl(
                    (&mut buff[size_of::<BlobMessage>()..(size_of::<BlobMessage>() * 2)])
                        .try_into()
                        .unwrap(),
                );
            }
        }
        let device = fixture.close().await;
        device.ensure_unique();

        device.reopen(false);
        let fixture = open_blob_fixture(device).await;
        {
            let mut state = new_profile_state(true);
            let volume = fixture.volume().volume();

            let replay_handle = Box::new(FakeReaderWriter::new());
            replay_handle.inner.lock().data = buff;

            state.replay_profile(
                replay_handle,
                volume.clone(),
                volume.scope().try_active_guard().unwrap(),
            );

            // Wait for the replay to populate the page.
            let blob = fixture.get_blob((*hash).into()).await.expect("Opening blob");
            while blob.vmo().info().unwrap().committed_bytes == 0 {
                fasync::Timer::new(Duration::from_millis(25)).await;
            }
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_replay_file_missing_or_tombstoned() {
        let fixture = TestFixture::new().await;
        let mut buff = vec![0u8; IO_SIZE];
        // Create the blob that comes after the missing blob. Ensure it still gets
        // recorded.
        let remaining_file_id;
        let tombstoned_file_id;
        // First encode the file that is missing.
        {
            let id = write_file(&fixture, "foo", &[1, 2, 3, 4]).await;
            let message = FileMessage { id, offset: 0 };
            message.encode_to_impl((&mut buff[0..size_of::<FileMessage>()]).try_into().unwrap());
        }
        // Remove the file now.
        fixture
            .root()
            .unlink("foo", &fio::UnlinkOptions::default())
            .await
            .unwrap()
            .expect("Unlinking");

        // Encode the file that will be tombstoned during replay.
        {
            tombstoned_file_id = write_file(&fixture, "bar", &[1, 2, 3, 4]).await;
            let message = FileMessage { id: tombstoned_file_id, offset: 0 };
            message.encode_to_impl(
                (&mut buff[size_of::<FileMessage>()..(size_of::<FileMessage>() * 2)])
                    .try_into()
                    .unwrap(),
            );
        }

        // Encode the file that will remain and be replayed last.
        {
            remaining_file_id = write_file(&fixture, "baz", &[1, 2, 3, 4]).await;
            let message = FileMessage { id: remaining_file_id, offset: 0 };
            message.encode_to_impl(
                (&mut buff[(size_of::<FileMessage>() * 2)..(size_of::<FileMessage>() * 3)])
                    .try_into()
                    .unwrap(),
            );
        }
        let device = fixture.close().await;
        device.ensure_unique();

        device.reopen(false);
        let fixture =
            TestFixture::open(device, TestFixtureOptions { format: false, ..Default::default() })
                .await;
        {
            // Get a ref to the Arc on the file, then unlink it. Since the open count is zero it
            // should get marked for tombstone right away.
            let tombstoned_file = fixture
                .volume()
                .volume()
                .get_or_load_node(tombstoned_file_id, ObjectDescriptor::File, None)
                .await
                .expect("Opening file object")
                .into_any()
                .downcast::<FxFile>()
                .unwrap();
            fixture
                .root()
                .unlink("bar", &fio::UnlinkOptions::default())
                .await
                .unwrap()
                .expect("Unlinking");

            let mut state = new_profile_state(false);
            let volume = fixture.volume().volume();

            let replay_handle = Box::new(FakeReaderWriter::new());
            replay_handle.inner.lock().data = buff;

            state.replay_profile(
                replay_handle,
                volume.clone(),
                volume.scope().try_active_guard().unwrap(),
            );

            // Wait for the replay to populate the page.
            let remaining_file = fixture
                .volume()
                .volume()
                .get_or_load_node(remaining_file_id, ObjectDescriptor::File, None)
                .await
                .expect("Opening file object")
                .into_any()
                .downcast::<FxFile>()
                .unwrap();
            while remaining_file.vmo().info().unwrap().committed_bytes == 0 {
                fasync::Timer::new(Duration::from_millis(25)).await;
            }

            // The tombstoned file should not have anything committed because it shouldn't be able
            // to open.
            assert_eq!(tombstoned_file.vmo().info().unwrap().committed_bytes, 0);
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_recording_stopped_by_scope_shutdown() {
        let fixture = TestFixture::new().await;
        {
            let volumes_directory = fixture.volumes_directory();
            let mut state = new_profile_state(false);
            // The recorder outlives the shutdown here, to show that we stopped the recording due to
            // scope shutdown.
            let (store_id, _recorder) = {
                let volume_and_root = volumes_directory
                    .create_and_mount_volume("other_volume", None, false, CreateOptions::default())
                    .await
                    .unwrap();
                let volume = volume_and_root.into_volume();

                let handle =
                    FileRecordingHandle::new(TEST_PROFILE_NAME, volume.clone()).await.unwrap();
                (volume.store().store_object_id(), state.record_new(&volume, Box::new(handle)))
            };
            volumes_directory.lock().await.unmount(store_id).await.expect("Unmounting");
            state.wait_for_recording_to_finish().await;

            let volume_and_root =
                volumes_directory.mount_volume("other_volume", None, false).await.unwrap();
            let volume = volume_and_root.into_volume();
            let profile_dir = volume.get_profile_directory().await.unwrap();
            assert!(profile_dir.lookup(TEST_PROFILE_NAME).await.unwrap().is_none());
        }
        fixture.close().await;
    }
}

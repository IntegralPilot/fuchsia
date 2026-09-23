// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::fuchsia::errors::map_to_status;
use crate::fuchsia::file::FxFile;
use crate::fuchsia::node::OpenedNode;
use anyhow::Error;
use block_client::{BlockFifoRequest, BlockFifoResponse};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::{self as block, BlockMarker, BlockRequest};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::stream::TryStreamExt;
use futures::try_join;
use fxfs::errors::FxfsError;
use fxfs::round::{round_down, round_up};
use rustc_hash::FxHashMap as HashMap;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use vfs::file::File;
use vfs::node::Node;

// Multiple Block I/O request may be sent as a group.
// Notes:
// - the group is identified by `group_id` in the request
// - if using groups, a response will not be sent unless `BlockIoFlag::GROUP_LAST`
//   flag is set.
// - when processing a request of a group fails, subsequent requests of that
//   group will not be processed.
//
// Refer to sdk/fidl/fuchsia.hardware.block.driver/block.fidl for details.
//
// FifoMessageGroup keeps track of the relevant BlockFifoResponse field for
// a group requests. Only `status` and `count` needs to be updated.
struct FifoMessageGroup {
    group_id: u16,
    status: zx::sys::zx_status_t,
    count: u32,
}

impl Hash for FifoMessageGroup {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.group_id.hash(state);
    }
}

impl FifoMessageGroup {
    // Initialise a FifoMessageGroup given the request group ID.
    // `count` is set to 0 as no requests has been processed yet.
    fn new(group_id: u16) -> Self {
        Self { group_id, status: zx::sys::ZX_OK, count: 0 }
    }

    // Takes the FifoMessageGroup and converts it to a BlockFifoResponse.
    // Note that this doesn't return the request ID, it needs to be set
    // after extracting the BlockFifoResponse before sending it
    fn into_response(self) -> BlockFifoResponse {
        return BlockFifoResponse {
            status: self.status,
            group: self.group_id,
            count: self.count,
            ..Default::default()
        };
    }

    fn increment_count(&mut self) {
        self.count += 1;
    }

    fn set_status(&mut self, status: zx::sys::zx_status_t) {
        self.status = status;
    }

    fn is_err(&self) -> bool {
        self.status != zx::sys::ZX_OK
    }
}

struct FifoMessageGroups(HashMap<u16, FifoMessageGroup>);

// Keeps track of all the group requests that are currently being processed
impl FifoMessageGroups {
    fn new() -> Self {
        Self(HashMap::default())
    }

    // Returns the current MessageGroup with this group ID
    fn get(&mut self, group_id: u16) -> &mut FifoMessageGroup {
        self.0.entry(group_id).or_insert_with(|| FifoMessageGroup::new(group_id))
    }

    // Remove a group when `BlockIoFlag::GROUP_LAST` flag is set.
    fn remove(&mut self, group_id: u16) -> FifoMessageGroup {
        match self.0.remove(&group_id) {
            Some(group) => group,
            // `remove(group_id)` can be called when the group has not yet been
            // added to this FifoMessageGroups. In which case, return a default
            // MessageGroup.
            None => FifoMessageGroup::new(group_id),
        }
    }
}

// This is the default slice size used for Volumes in devices
const DEVICE_VOLUME_SLICE_SIZE: u64 = 32 * 1024;

/// Implements server to handle Block requests
pub struct BlockServer {
    file: OpenedNode<FxFile>,
    server_channel: Option<zx::Channel>,
    maybe_server_fifo: Mutex<Option<zx::Fifo<BlockFifoResponse, BlockFifoRequest>>>,
    message_groups: Mutex<FifoMessageGroups>,
    vmos: Mutex<BTreeMap<u16, zx::Vmo>>,
}

impl BlockServer {
    /// Creates a new BlockServer given a server channel to listen on.
    pub fn new(file: OpenedNode<FxFile>, server_channel: zx::Channel) -> BlockServer {
        BlockServer {
            file,
            server_channel: Some(server_channel),
            maybe_server_fifo: Mutex::new(None),
            message_groups: Mutex::new(FifoMessageGroups::new()),
            vmos: Mutex::new(BTreeMap::new()),
        }
    }

    // Returns a VMO id that is currently not being used
    fn get_vmo_id(&self, vmo: zx::Vmo) -> Option<u16> {
        let mut vmos = self.vmos.lock();
        let mut prev_id = 0;
        for &id in vmos.keys() {
            if id != prev_id + 1 {
                let vmo_id = prev_id + 1;
                vmos.insert(vmo_id, vmo);
                return Some(vmo_id);
            }
            prev_id = id;
        }
        if prev_id < u16::MAX {
            let vmo_id = prev_id + 1;
            vmos.insert(vmo_id, vmo);
            Some(vmo_id)
        } else {
            None
        }
    }

    async fn handle_blockio_write(&self, request: &BlockFifoRequest) -> Result<(), Error> {
        let block_size = self.file.get_block_size();

        let data = {
            let vmos = self.vmos.lock();
            let vmo = vmos.get(&request.vmoid).ok_or(FxfsError::NotFound)?;
            let mut buffer = vec![0u8; (request.length as u64 * block_size) as usize];
            vmo.read(&mut buffer[..], request.vmo_offset * block_size)?;
            buffer
        };

        self.file.write_at_uncached(request.dev_offset * block_size, &data[..]).await?;

        Ok(())
    }

    async fn handle_blockio_read(&self, request: &BlockFifoRequest) -> Result<(), Error> {
        let block_size = self.file.get_block_size();

        let mut buffer = vec![0u8; (request.length as u64 * block_size) as usize];
        let bytes_read =
            self.file.read_at_uncached(request.dev_offset * block_size, &mut buffer[..]).await?;

        // Fill in the rest of the buffer if bytes_read is less than the requested amount
        buffer[bytes_read as usize..].fill(0);

        let vmos = self.vmos.lock();
        let vmo = vmos.get(&request.vmoid).ok_or(FxfsError::NotFound)?;
        vmo.write(&buffer[..], request.vmo_offset * block_size)?;

        Ok(())
    }

    async fn process_fifo_request(&self, request: &BlockFifoRequest) -> zx::sys::zx_status_t {
        fn into_raw_status(result: Result<(), Error>) -> zx::sys::zx_status_t {
            zx::Status::result_into_raw(result.map_err(map_to_status))
        }

        match block_client::BlockOpcode::from_primitive(request.command.opcode) {
            Some(block_client::BlockOpcode::CloseVmo) => {
                let mut vmos = self.vmos.lock();
                match vmos.remove(&request.vmoid) {
                    Some(_vmo) => zx::sys::ZX_OK,
                    None => zx::sys::ZX_ERR_NOT_FOUND,
                }
            }
            Some(block_client::BlockOpcode::Write) => {
                into_raw_status(self.handle_blockio_write(&request).await)
            }
            Some(block_client::BlockOpcode::Read) => {
                into_raw_status(self.handle_blockio_read(&request).await)
            }
            // TODO(https://fxbug.dev/293970391): simply returning ZX_OK since we're
            // writing to device and no need to flush cache, but need to
            // check that flush goes down the stack
            Some(block_client::BlockOpcode::Flush) => zx::sys::ZX_OK,
            // TODO(https://fxbug.dev/293970391)
            Some(block_client::BlockOpcode::Trim) => zx::sys::ZX_OK,
            None => panic!("Unexpected message, request {:?}", request.command.opcode),
        }
    }

    async fn handle_fifo_request(&self, request: BlockFifoRequest) -> Option<BlockFifoResponse> {
        let flags = block_client::BlockIoFlag::from_bits_truncate(request.command.flags);
        let is_group = flags.contains(block_client::BlockIoFlag::GROUP_ITEM);
        let wants_reply = flags.contains(block_client::BlockIoFlag::GROUP_LAST);

        // Set up the BlockFifoResponse for this request, but do no process request yet
        let mut maybe_reply = {
            if is_group {
                let mut groups = self.message_groups.lock();
                if wants_reply {
                    let mut group = groups.remove(request.group);
                    group.increment_count();

                    // This occurs when a previous request in this group has failed
                    if group.is_err() {
                        let mut reply = group.into_response();
                        reply.reqid = request.reqid;
                        // No need to process this request
                        return Some(reply);
                    }

                    let mut response = group.into_response();
                    response.reqid = request.reqid;
                    Some(response)
                } else {
                    let group = groups.get(request.group);
                    group.increment_count();

                    if group.is_err() {
                        // No need to process this request
                        return None;
                    }
                    None
                }
            } else {
                Some(BlockFifoResponse { reqid: request.reqid, count: 1, ..Default::default() })
            }
        };
        let status = self.process_fifo_request(&request).await;
        // Status only needs to be updated in the reply if it's not OK
        if status != zx::sys::ZX_OK {
            match &mut maybe_reply {
                None => {
                    // maybe_reply will only be None if it's part of a group request
                    self.message_groups.lock().get(request.group).set_status(status);
                }
                Some(reply) => {
                    reply.status = status;
                }
            }
        }
        maybe_reply
    }

    async fn handle_request(&self, request: BlockRequest) -> Result<(), Error> {
        match request {
            BlockRequest::GetInfo { responder } => {
                let block_size = self.file.get_block_size();
                let block_count =
                    block_size.align_up_to_blocks(self.file.get_size().await.unwrap());
                responder.send(Ok(&block::BlockInfo {
                    block_count,
                    block_size: block_size.get() as u32,
                    max_transfer_size: 1024 * 1024,
                    flags: block::DeviceFlag::empty(),
                }))?;
            }
            BlockRequest::OpenSession { session, control_handle: _ } => {
                let stream = session.into_stream();
                let () = stream
                    .try_for_each(|request| async {
                        let () = match request {
                            block::SessionRequest::GetFifo { responder } => {
                                match self.maybe_server_fifo.lock().take() {
                                    Some(fifo) => responder.send(Ok(fifo.downcast()))?,
                                    None => {
                                        responder.send(Err(zx::Status::NO_RESOURCES.into_raw()))?
                                    }
                                }
                            }
                            block::SessionRequest::AttachVmo { vmo, responder } => {
                                match vmo.info() {
                                    Ok(info)
                                        if info.flags.contains(zx::VmoInfoFlags::RESIZABLE) =>
                                    {
                                        responder.send(Err(zx::Status::INVALID_ARGS.into_raw()))?;
                                    }
                                    Ok(_) => match self.get_vmo_id(vmo) {
                                        Some(vmo_id) => {
                                            responder.send(Ok(&block::VmoId { id: vmo_id }))?
                                        }
                                        None => responder
                                            .send(Err(zx::Status::NO_RESOURCES.into_raw()))?,
                                    },
                                    Err(status) => {
                                        responder.send(Err(status.into_raw()))?;
                                    }
                                }
                            }
                            // TODO(https://fxbug.dev/293970391): close fifo
                            block::SessionRequest::Close { responder } => responder.send(Ok(()))?,
                        };
                        Ok(())
                    })
                    .await?;
            }
            BlockRequest::OpenSessionWithOptions { session, mappings: _, control_handle: _ } => {
                session.close_with_epitaph(zx::Status::NOT_SUPPORTED)?;
            }
            BlockRequest::ConnectMapper { server_end: _, responder } => {
                responder.send(Err(zx::Status::NOT_SUPPORTED.into_raw()))?;
            }
            // TODO(https://fxbug.dev/293970391)
            BlockRequest::GetTypeGuid { responder } => {
                responder.send(zx::sys::ZX_ERR_NOT_SUPPORTED, None)?;
            }
            // TODO(https://fxbug.dev/293970391)
            BlockRequest::GetInstanceGuid { responder } => {
                responder.send(zx::sys::ZX_ERR_NOT_SUPPORTED, None)?;
            }
            // TODO(https://fxbug.dev/293970391)
            BlockRequest::GetName { responder } => {
                responder.send(zx::sys::ZX_ERR_NOT_SUPPORTED, None)?;
            }
            // TODO(https://fxbug.dev/293970391)
            BlockRequest::GetMetadata { responder } => {
                responder.send(Err(zx::sys::ZX_ERR_NOT_SUPPORTED))?;
            }
            BlockRequest::QuerySlices { start_slices, responder } => {
                // Initialise slices with default value.
                let default = block::VsliceRange { allocated: false, count: 0 };
                let mut slices = [default; block::MAX_SLICE_REQUESTS as usize];

                let mut status = zx::sys::ZX_OK;
                let mut response_count = 0;
                for (slice, start_slice) in slices.iter_mut().zip(start_slices.into_iter()) {
                    match self.file.is_allocated(start_slice * DEVICE_VOLUME_SLICE_SIZE).await {
                        Ok((allocated, bytes)) => {
                            slice.count = round_up(bytes, DEVICE_VOLUME_SLICE_SIZE).unwrap();
                            slice.allocated = allocated;
                            response_count += 1;
                        }
                        Err(e) => {
                            status = e.into_raw();
                            break;
                        }
                    }
                }
                responder.send(status, &slices, response_count)?;
            }
            // TODO(https://fxbug.dev/293970391): Ensure this returns the correct information.
            BlockRequest::GetVolumeInfo { responder } => {
                match self.file.get_attributes(fio::NodeAttributesQuery::STORAGE_SIZE).await {
                    Ok(attr) => {
                        debug_assert!(attr.immutable_attributes.storage_size.is_some());
                        let allocated_bytes = attr.immutable_attributes.storage_size.unwrap();
                        let unallocated_bytes = self.file.get_size_uncached() - allocated_bytes;
                        let allocated_slices =
                            round_up(allocated_bytes, DEVICE_VOLUME_SLICE_SIZE).unwrap();
                        let unallocated_bytes =
                            round_down(unallocated_bytes, DEVICE_VOLUME_SLICE_SIZE);
                        let manager = block::VolumeManagerInfo {
                            slice_size: DEVICE_VOLUME_SLICE_SIZE,
                            slice_count: allocated_slices + unallocated_bytes,
                            assigned_slice_count: allocated_slices,
                            maximum_slice_count: allocated_slices + unallocated_bytes,
                            max_virtual_slice: allocated_slices + unallocated_bytes,
                        };
                        let volume_info = block::VolumeInfo {
                            partition_slice_count: allocated_slices,
                            slice_limit: 0,
                        };
                        responder.send(zx::sys::ZX_OK, Some(&manager), Some(&volume_info))?;
                    }
                    Err(e) => {
                        responder.send(e.into_raw(), None, None)?;
                    }
                }
            }
            BlockRequest::Extend { start_slice, slice_count, responder } => {
                // TODO(https://fxbug.dev/293970391): This is a hack! When extend is called, the
                // extent is expected to be set as allocated. The easiest way to do this is to
                // just write an extent of zeroed data. Another issue the size: the memory
                // allocated here should be bounded to what's available.
                let data = vec![0u8; (slice_count * DEVICE_VOLUME_SLICE_SIZE) as usize];
                match self
                    .file
                    .write_at_uncached(start_slice * DEVICE_VOLUME_SLICE_SIZE, data[..].into())
                    .await
                {
                    Ok(_) => responder.send(zx::sys::ZX_OK)?,
                    Err(status) => responder.send(status.into_raw())?,
                };
            }
            // TODO(https://fxbug.dev/293970391)
            BlockRequest::Shrink { start_slice: _, slice_count: _, responder } => {
                responder.send(zx::sys::ZX_OK)?;
            }
            // TODO(https://fxbug.dev/293970391)
            BlockRequest::Destroy { responder } => {
                responder.send(zx::sys::ZX_OK)?;
            }
        }
        Ok(())
    }

    async fn handle_requests(&self, server: ServerEnd<BlockMarker>) -> Result<(), Error> {
        server
            .into_stream()
            .map_err(|e| e.into())
            .try_for_each_concurrent(None, |request| self.handle_request(request))
            .await?;
        Ok(())
    }

    pub async fn run(mut self) {
        let server = ServerEnd::<BlockMarker>::new(self.server_channel.take().unwrap());

        // Create a fifo pair
        let (server_fifo, client_fifo) =
            match zx::Fifo::<BlockFifoRequest, BlockFifoResponse>::create(16) {
                Ok(endpoints) => endpoints,
                Err(e) => {
                    log::error!("Failed to create fifo for block requests: {:?}", e);
                    return;
                }
            };
        self.maybe_server_fifo = Mutex::new(Some(client_fifo));

        // Handling requests from fifo
        let fifo_future = async {
            let mut fifo = fasync::Fifo::from_fifo(server_fifo);
            let (mut reader, mut writer) = fifo.async_io();
            let mut request = BlockFifoRequest::default();
            loop {
                match reader.read_entries(&mut request).await {
                    Ok(_) => {
                        if let Some(response) = self.handle_fifo_request(request).await {
                            writer.write_entries(&response).await?;
                        }
                        // if `self.handle_fifo_request(..)` returns None, then
                        // there's no reply for this request. This occurs for
                        // requests part of a group request where
                        // `BlockIoFlag::GROUP_LAST` flag is not set.
                    }
                    Err(zx::Status::PEER_CLOSED) => break Result::<_, Error>::Ok(()),
                    Err(e) => break Err(e.into()),
                }
            }
        };

        // Handling requests from fidl
        let channel_future = async {
            self.handle_requests(server).await?;
            // This is temporary for when client doesn't call for fifo
            self.maybe_server_fifo.lock().take();
            Ok(())
        };

        let _ = try_join!(fifo_future, channel_future);
    }
}

#[cfg(test)]
mod tests {
    use crate::fuchsia::testing::{TestFixture, open_file_checked};
    use block_client::{BlockClient, BlockIoFlag, BlockOpcode, RemoteBlockClient, VmoId};
    use block_protocol::{BlockFifoCommand, BlockFifoRequest, BlockFifoResponse};
    use fidl::endpoints::ServerEnd;
    use fidl_fuchsia_fxfs::{
        BlobCreatorMarker, BlobReaderMarker, FileBackedVolumeProviderMarker,
        FileBackedVolumeProviderProxy,
    };
    use fidl_fuchsia_io::{self as fio};
    use fidl_fuchsia_storage_block::{BlockMarker, SessionMarker};
    use fs_management::Blobfs;
    use fs_management::filesystem::{BlockConnector as _, Filesystem};
    use fuchsia_async as fasync;
    use futures::join;
    use rustc_hash::FxHashSet as HashSet;

    const TEST_DEVICE_FILE_SIZE: u64 = 4 * 1024 * 1024;
    const TEST_DEVICE_FILE_NAME: &'static str = "block_device";

    struct TestFileBackedVolume {
        proxy: FileBackedVolumeProviderProxy,
        token: zx::NullableHandle,
    }

    impl TestFileBackedVolume {
        async fn new(fixture: &TestFixture) -> Self {
            let proxy = fuchsia_component_client::connect_to_protocol_at_dir_svc::<
                FileBackedVolumeProviderMarker,
            >(fixture.volume_out_dir())
            .unwrap();
            let (status, token) = fixture.root().get_token().await.expect("FIDL error");
            zx::ok(status).expect("get_token error");
            Self { proxy, token: token.unwrap() }
        }
    }

    impl fs_management::filesystem::BlockConnector for TestFileBackedVolume {
        fn connect_channel_to_block(
            &self,
            server_end: ServerEnd<BlockMarker>,
        ) -> Result<(), anyhow::Error> {
            self.proxy
                .open(
                    self.token.duplicate_handle(zx::Rights::SAME_RIGHTS)?,
                    TEST_DEVICE_FILE_NAME,
                    server_end.into_channel().into(),
                )
                .expect("open failed");
            Ok(())
        }
    }

    async fn serve_test_file_backed_volume(fixture: &TestFixture) -> TestFileBackedVolume {
        let file = open_file_checked(
            fixture.root(),
            TEST_DEVICE_FILE_NAME,
            fio::Flags::FLAG_MAYBE_CREATE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
        )
        .await;
        file.resize(TEST_DEVICE_FILE_SIZE).await.expect("FIDL error").expect("resize error");
        let () = file.close().await.expect("FIDL error").expect("close error");
        TestFileBackedVolume::new(&fixture).await
    }

    #[fuchsia::test(threads = 10)]
    async fn test_block_server() {
        let fixture = TestFixture::new().await;
        let volume = serve_test_file_backed_volume(&fixture).await;

        {
            let mut blobfs = Filesystem::new(volume, Blobfs::default());
            blobfs.format().await.expect("format blobfs failed");
            blobfs.fsck().await.expect("fsck failed");
        }
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_attach_vmo() {
        let fixture = TestFixture::new().await;
        let (volume, server) = fidl::endpoints::create_proxy::<BlockMarker>();
        join!(
            async {
                let block_client = RemoteBlockClient::new(volume).await.unwrap();
                let mut vmo_set = HashSet::default();
                let vmo = zx::Vmo::create(1).unwrap();
                for _ in 1..5 {
                    // SAFETY: Test code. We are testing attaching the same VMO multiple times.
                    // No I/O is performed on this VMO in this test, so no UB is possible.
                    match unsafe { block_client.attach_vmo(&vmo) }.await {
                        Ok(vmo_id) => {
                            // Make sure that vmo_id is unique.
                            // TODO(https://fxbug.dev/293970391): Need to detach vmoid. Calling
                            // into_id() is a temporary solution. Remove this after detaching vmo
                            // has been implemented.
                            assert_eq!(vmo_set.insert(vmo_id.into_id()), true);
                        }
                        Err(e) => panic!("unexpected error {:?}", e),
                    }
                }
            },
            async {
                let provider = serve_test_file_backed_volume(&fixture).await;
                provider.connect_channel_to_block(server).unwrap();
            }
        );
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_attach_resizable_vmo() {
        let fixture = TestFixture::new().await;
        let (volume, server) = fidl::endpoints::create_proxy::<BlockMarker>();
        join!(
            async {
                let block_client = RemoteBlockClient::new(volume).await.unwrap();
                let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, 4096).unwrap();
                // SAFETY: Test code. Only attach once, no other mappings.
                let status = unsafe { block_client.attach_vmo(&vmo) }.await.unwrap_err();
                assert_eq!(status, zx::Status::INVALID_ARGS);
            },
            async {
                let provider = serve_test_file_backed_volume(&fixture).await;
                provider.connect_channel_to_block(server).unwrap();
            }
        );
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_detach_vmo() {
        let fixture = TestFixture::new().await;
        let (volume, server) = fidl::endpoints::create_proxy::<BlockMarker>();
        join!(
            async {
                let block_client = RemoteBlockClient::new(volume).await.unwrap();
                let vmo = zx::Vmo::create(1).unwrap();
                // SAFETY: Test code. Only attach once, no other mappings.
                let vmo_id =
                    unsafe { block_client.attach_vmo(&vmo) }.await.expect("attach_vmo failed");
                let vmo_id_copy = VmoId::new(vmo_id.id());
                block_client.detach_vmo(vmo_id).await.expect("detach failed");
                block_client.detach_vmo(vmo_id_copy).await.expect_err("detach succeeded");
            },
            async {
                let provider = serve_test_file_backed_volume(&fixture).await;
                provider.connect_channel_to_block(server).unwrap();
            }
        );
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_read_write_files() {
        let fixture = TestFixture::new().await;
        let (volume, server) = fidl::endpoints::create_proxy::<BlockMarker>();
        join!(
            async {
                let block_client = RemoteBlockClient::new(volume).await.unwrap();
                let vmo = zx::Vmo::create(131072).unwrap();
                // SAFETY: Test code. Only attach once, no other mappings.
                let vmo_id =
                    unsafe { block_client.attach_vmo(&vmo) }.await.expect("attach_vmo failed");

                // Must write with length as a multiple of the block_size
                let offset = block_client.block_size() as usize;
                let len = block_client.block_size() as usize;
                let write_buf = vec![0xa3u8; len];
                block_client
                    .write_at(write_buf[..].into(), offset as u64)
                    .await
                    .expect("write_at failed");

                // Read back an extra block either side
                let mut read_buf = vec![0u8; len + 2 * block_client.block_size() as usize];
                block_client
                    .read_at(read_buf.as_mut_slice().into(), 0)
                    .await
                    .expect("read_at failed");

                // We expect the extra block on the LHS of the read_buf to be 0
                assert_eq!(&read_buf[..offset], &vec![0; offset][..]);

                assert_eq!(&read_buf[offset..offset + len], &write_buf);

                // We expect the extra block on the RHS of the read_buf to be 0
                assert_eq!(
                    &read_buf[offset + len..],
                    &vec![0; block_client.block_size() as usize][..]
                );

                block_client.detach_vmo(vmo_id).await.expect("detach failed");
            },
            async {
                let provider = serve_test_file_backed_volume(&fixture).await;
                provider.connect_channel_to_block(server).unwrap();
            }
        );
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_flush_is_called() {
        let fixture = TestFixture::new().await;
        let (volume, server) = fidl::endpoints::create_proxy::<BlockMarker>();
        join!(
            async {
                let block_client = RemoteBlockClient::new(volume).await.unwrap();
                block_client.flush().await.expect("flush failed");
            },
            async {
                let provider = serve_test_file_backed_volume(&fixture).await;
                provider.connect_channel_to_block(server).unwrap();
            }
        );
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_get_info() {
        let fixture = TestFixture::new().await;
        let (volume, server) = fidl::endpoints::create_proxy::<BlockMarker>();
        join!(
            async {
                let info = volume
                    .get_info()
                    .await
                    .expect("get_info failed")
                    .map_err(zx::Status::err_from_raw)
                    .expect("block get_info failed");
                assert_eq!(info.block_count * u64::from(info.block_size), TEST_DEVICE_FILE_SIZE);
            },
            async {
                let provider = serve_test_file_backed_volume(&fixture).await;
                provider.connect_channel_to_block(server).unwrap();
            }
        );
        fixture.close().await;
    }

    #[fuchsia::test(threads = 10)]
    async fn test_blobfs() {
        let fixture = TestFixture::new().await;
        let volume = serve_test_file_backed_volume(&fixture).await;

        {
            let content = b"Hello world!";
            let merkle_root_hash = fuchsia_merkle::root_from_slice(content);
            let delivery_blob =
                delivery_blob::Type1Blob::generate(content, delivery_blob::CompressionMode::Never);
            let mut blobfs = Filesystem::new(volume, Blobfs::default());
            blobfs.format().await.expect("format blobfs failed");
            blobfs.fsck().await.expect("fsck failed");
            // Mount blobfs
            let serving = blobfs.serve().await.expect("serve blobfs failed");

            let creator = fuchsia_component_client::connect_to_protocol_at_dir_root::<
                BlobCreatorMarker,
            >(serving.exposed_dir())
            .unwrap();
            let writer = creator.create(&merkle_root_hash.into(), false).await.unwrap().unwrap();
            let mut writer =
                blob_writer::BlobWriter::create(writer.into_proxy(), delivery_blob.len() as u64)
                    .await
                    .unwrap();
            writer.write(&delivery_blob).await.unwrap();
            // Check that blobfs can be successfully unmounted
            serving.shutdown().await.expect("shutdown blobfs failed");

            // Re-open Blobfs and verify the contents persisted.
            let volume = TestFileBackedVolume::new(&fixture).await;

            let blobfs = Filesystem::new(volume, Blobfs { readonly: true, ..Default::default() });
            let serving = blobfs.serve().await.expect("serve blobfs failed");
            {
                let reader = fuchsia_component_client::connect_to_protocol_at_dir_root::<
                    BlobReaderMarker,
                >(serving.exposed_dir())
                .unwrap();
                let vmo = reader.get_vmo(&merkle_root_hash.into()).await.unwrap().unwrap();
                let read_content = vmo.read_to_vec::<u8>(0, content.len() as u64).unwrap();
                assert_eq!(read_content, content);
            }

            serving.shutdown().await.expect("shutdown blobfs failed");
        }
        fixture.close().await;
    }

    #[fuchsia::test]
    async fn test_groups() {
        let fixture = TestFixture::new().await;
        let (volume, server) = fidl::endpoints::create_proxy::<BlockMarker>();
        let provider = serve_test_file_backed_volume(&fixture).await;
        provider.connect_channel_to_block(server).unwrap();

        let (session_proxy, server) = fidl::endpoints::create_proxy::<SessionMarker>();
        volume.open_session(server).unwrap();

        let vmo = zx::Vmo::create(storage_units::PAGE_SIZE.get()).unwrap();
        let vmo_id = session_proxy
            .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();

        let mut fifo = fasync::Fifo::from_fifo(session_proxy.get_fifo().await.unwrap().unwrap());
        let (mut reader, mut writer) = fifo.async_io();

        writer
            .write_entries(&BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: BlockIoFlag::GROUP_ITEM.bits(),
                    ..Default::default()
                },
                group: 1,
                vmoid: vmo_id.id,
                length: 1,
                ..Default::default()
            })
            .await
            .unwrap();

        writer
            .write_entries(&BlockFifoRequest {
                command: BlockFifoCommand {
                    opcode: BlockOpcode::Read.into_primitive(),
                    flags: (BlockIoFlag::GROUP_ITEM | BlockIoFlag::GROUP_LAST).bits(),
                    ..Default::default()
                },
                reqid: 2,
                group: 1,
                vmoid: vmo_id.id,
                length: 1,
                ..Default::default()
            })
            .await
            .unwrap();

        let mut response = BlockFifoResponse::default();
        reader.read_entries(&mut response).await.unwrap();
        assert_eq!(response.status, zx::sys::ZX_OK);
        assert_eq!(response.reqid, 2);
        assert_eq!(response.group, 1);

        fixture.close().await;
    }
}

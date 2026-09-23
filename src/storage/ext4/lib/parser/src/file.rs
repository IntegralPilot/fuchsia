// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ext4_lib::parser::{VmoWriter, XattrMap};
use ext4_lib::processor::Ext4Processor;
use ext4_lib::structs::ParsingError;
use fidl_fuchsia_io as fio;
use fuchsia_sync::Mutex;
use std::sync::{Arc, Weak};
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::execution_scope::ExecutionScope;
use vfs::file::{
    FidlIoConnection, File, FileIo, FileLike, FileOptions, GetVmo, StreamIoConnection, SyncMode,
};
use vfs::node::Node;
use vfs::{ObjectRequestRef, immutable_attributes};
use zx::{Status, Vmo};

use crate::types::ExtAttributes;

/// An ext4 filesystem file node.
pub struct ExtFile {
    inode: u64,
    attributes: ExtAttributes,
    xattrs: XattrMap,
    processor: Arc<Ext4Processor>,
    read_only: bool,
    connection: Mutex<Option<Weak<OpenExtFile>>>,
}

impl std::fmt::Debug for ExtFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtFile")
            .field("inode", &self.inode)
            .field("attributes", &self.attributes)
            .field("xattrs", &self.xattrs)
            .field("read_only", &self.read_only)
            .finish_non_exhaustive()
    }
}

impl ExtFile {
    fn new(
        processor: Arc<Ext4Processor>,
        inode: u64,
        attributes: ExtAttributes,
        xattrs: XattrMap,
        read_only: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            inode,
            attributes,
            xattrs,
            processor,
            read_only,
            connection: Default::default(),
        })
    }

    /// Creates a new [`ExtFile`] from the processor with the given inode number.
    pub fn from_processor(
        processor: Arc<Ext4Processor>,
        ino: u32,
        read_only: bool,
    ) -> Result<Arc<Self>, Status> {
        let inode = processor.inode(ino).map_err(|_| Status::INTERNAL)?;
        let attributes = ExtAttributes::from_inode(inode);
        let xattrs = processor.inode_xattrs(ino).map_err(|_| Status::INTERNAL)?;
        if !read_only && processor.read_only() {
            return Err(Status::INTERNAL);
        }
        Ok(Self::new(processor, ino.into(), attributes, xattrs, read_only))
    }

    fn get_or_create_connection(self: &Arc<Self>) -> Result<Arc<OpenExtFile>, Status> {
        let mut connection = self.connection.lock();
        if let Some(weak_conn) = &*connection {
            if let Some(conn) = weak_conn.upgrade() {
                return Ok(conn);
            }
        }

        let file_size =
            self.processor.file_size(self.inode as u32).map_err(|_| Status::INTERNAL)?;
        let mut writer = VmoWriter::new(file_size)?;
        self.processor.read_data_into(self.inode as u32, &mut writer).map_err(|err| {
            log::error!("failed to read file data: {err}");
            Status::IO
        })?;
        let vmo = writer.into_vmo();

        let conn = Arc::new(OpenExtFile { file: self.clone(), vmo: Arc::new(vmo) });
        *connection = Some(Arc::downgrade(&conn));
        Ok(conn)
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    #[cfg(test)]
    fn is_vmo_loaded(&self) -> bool {
        let conn = self.connection.lock();
        if let Some(weak) = &*conn { weak.upgrade().is_some() } else { false }
    }

    fn get_size(&self) -> Result<u64, Status> {
        self.processor.file_size(self.inode as u32).map_err(|_| Status::INTERNAL)
    }
}

impl GetEntryInfo for ExtFile {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(self.inode, fio::DirentType::File)
    }
}

impl DirectoryEntry for ExtFile {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_file(self)
    }
}

impl Node for ExtFile {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        let content_size = if requested_attributes.intersects(
            fio::NodeAttributesQuery::CONTENT_SIZE | fio::NodeAttributesQuery::STORAGE_SIZE,
        ) {
            Some(self.get_size()?)
        } else {
            None
        };

        Ok(self.attributes.overlay_node_attributes(
            requested_attributes,
            immutable_attributes!(
                requested_attributes,
                Immutable {
                    protocols: fio::NodeProtocolKinds::FILE,
                    abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::READ_BYTES,
                    content_size: content_size,
                    storage_size: content_size,
                    id: self.inode,
                }
            ),
        ))
    }

    async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, Status> {
        Ok(self.xattrs.keys().map(Clone::clone).collect())
    }

    async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, Status> {
        self.xattrs.get(&name).map(Clone::clone).ok_or(Status::NOT_FOUND)
    }
}

impl FileLike for ExtFile {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        options: FileOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        let connection = self.get_or_create_connection()?;
        if !self.read_only() {
            // Use a FidlIoConnection to manage writes. Note that reads will be slower because they
            // won't be using a stream.
            self.processor.record_open_metrics();
            FidlIoConnection::create_sync(scope, connection, options, object_request.take());
        } else {
            StreamIoConnection::create_sync(scope, connection, options, object_request.take());
        }
        Ok(())
    }
}

pub struct OpenExtFile {
    file: Arc<ExtFile>,
    vmo: Arc<Vmo>,
}

impl GetEntryInfo for OpenExtFile {
    fn entry_info(&self) -> EntryInfo {
        self.file.entry_info()
    }
}

impl Node for OpenExtFile {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        self.file.get_attributes(requested_attributes).await
    }

    async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, Status> {
        self.file.list_extended_attributes().await
    }

    async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, Status> {
        self.file.get_extended_attribute(name).await
    }
}

impl File for OpenExtFile {
    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        !self.file.read_only()
    }

    fn executable(&self) -> bool {
        false
    }

    async fn open_file(&self, _options: &FileOptions) -> Result<(), Status> {
        Ok(())
    }

    async fn truncate(&self, length: u64) -> Result<(), Status> {
        // We only support truncating to 0.
        if length == 0 {
            self.file.processor.truncate_to_zero(self.file.inode as u32).map_err(|e| {
                log::warn!("Error truncating: {:?}", e);
                Status::INTERNAL
            })?;
            let size = self.vmo.get_size()?;
            self.vmo.set_stream_size(0)?;
            self.vmo.op_range(zx::VmoOp::DECOMMIT, 0, size)?;
            Ok(())
        } else {
            Err(Status::NOT_SUPPORTED)
        }
    }

    async fn get_backing_memory(&self, flags: fio::VmoFlags) -> Result<Vmo, Status> {
        // Logic here matches fuchsia.io requirements and matches what works for memfs.
        // Shared requests are satisfied by duplicating an handle, and private shares are
        // child VMOs.
        let vmo_rights = vmo_flags_to_rights(flags)
            | zx::Rights::BASIC
            | zx::Rights::MAP
            | zx::Rights::GET_PROPERTY;
        // Unless private sharing mode is specified, we always default to shared.
        if flags.contains(fio::VmoFlags::PRIVATE_CLONE) {
            get_as_private(&self.vmo, vmo_rights)
        } else {
            self.vmo.duplicate_handle(vmo_rights)
        }
    }

    async fn get_size(&self) -> Result<u64, Status> {
        self.file.get_size()
    }

    async fn update_attributes(
        &self,
        _attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn sync(&self, _mode: SyncMode) -> Result<(), Status> {
        self.file.processor.sync().map_err(|e| {
            log::warn!("Error syncing: {:?}", e);
            Status::INTERNAL
        })?;
        Ok(())
    }
}

// Trait required for `FidlIoConnection` to support write operations.
impl FileIo for OpenExtFile {
    async fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<u64, Status> {
        // TODO(https://fxbug.dev/479943428): When full write support is implemented, ensure read_at
        // handles potential race conditions with concurrent writes.
        let file_size = self.get_size().await?;
        self.file.processor.record_read_metrics();

        if offset >= file_size {
            return Ok(0);
        }

        let readable_bytes = std::cmp::min(buffer.len() as u64, file_size - offset);
        if readable_bytes == 0 {
            return Ok(0);
        }
        self.vmo.read(&mut buffer[..readable_bytes as usize], offset)?;
        Ok(readable_bytes)
    }

    async fn write_at(&self, offset: u64, content: &[u8]) -> Result<u64, Status> {
        // TODO(https://fxbug.dev/479943428): This only supports overwriting file contents. It is a
        // a basic implementation - expanding the write support will require adding an allocator
        // and journalling.

        // We can only write to the pre-allocated file contents.
        match self.file.processor.overwrite_file_contents(self.file.inode as u32, content, offset) {
            Ok(_) => {
                self.vmo.write(content, offset)?;
            }
            Err(ParsingError::NotSupported(msg)) => {
                log::warn!("Failed to overwrite ({} not supported)", msg);
                return Err(Status::NOT_SUPPORTED);
            }
            Err(error) => {
                log::warn!(error:?; "Failed to overwrite");
                return Err(Status::INTERNAL);
            }
        }
        Ok(content.len() as u64)
    }

    async fn append(&self, _content: &[u8]) -> Result<(u64, u64), Status> {
        // TODO(https://fxbug.dev/479943428): Implement support.
        Err(Status::NOT_SUPPORTED)
    }
}

// Required by `StreamIoConnection`.
impl GetVmo for OpenExtFile {
    fn get_vmo(&self) -> &Vmo {
        &self.vmo
    }
}

fn get_as_private(vmo: &zx::Vmo, mut rights: zx::Rights) -> Result<Vmo, Status> {
    const CHILD_OPTIONS: zx::VmoChildOptions =
        zx::VmoChildOptions::REFERENCE.union(zx::VmoChildOptions::NO_WRITE);

    // Allow for the child VMO's content size and name to be changed.
    rights |= zx::Rights::SET_PROPERTY;

    let new_vmo = vmo.create_child(CHILD_OPTIONS, 0, 0)?;
    new_vmo.replace_handle(rights)
}

/// Maps VMO flags to their respective rights.
fn vmo_flags_to_rights(vmo_flags: fio::VmoFlags) -> zx::Rights {
    let mut rights = zx::Rights::NONE;
    if vmo_flags.contains(fio::VmoFlags::READ) {
        rights |= zx::Rights::READ;
    }
    if vmo_flags.contains(fio::VmoFlags::WRITE) {
        rights |= zx::Rights::WRITE;
    }
    if vmo_flags.contains(fio::VmoFlags::EXECUTE) {
        rights |= zx::Rights::EXECUTE;
    }
    rights
}

#[cfg(test)]
mod tests {
    use vfs::object_request::ToObjectRequest;

    pub fn serve_proxy(file: Arc<ExtFile>, flags: fio::Flags) -> fio::FileProxy {
        let scope = ExecutionScope::new();
        let (proxy, server) = fidl::endpoints::create_proxy::<fio::FileMarker>();
        let request = flags.to_object_request(server);
        request.handle(|request| vfs::file::serve(file, scope, &flags, request));
        proxy
    }

    use fidl_fuchsia_io::ExtendedAttributeValue;

    use super::*;
    use ext4_lib::readers::BlockDeviceReader;
    use fidl_fuchsia_storage_block as fblock;
    use fuchsia_async as fasync;
    use std::path::Path;
    use test_case::test_case;
    use test_vmo_backed_block_server::VmoBackedServer;

    const BLOCK_SIZE: u32 = 512;

    fn create_test_processor(read_only: bool) -> Arc<Ext4Processor> {
        let server = VmoBackedServer::from_file(BLOCK_SIZE, "/pkg/data/1file.img");
        let (block_client_end, block_server_end) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        std::thread::spawn(move || {
            let mut executor = fasync::TestExecutor::new();
            let _task = executor.run_singlethreaded(server.serve(block_server_end.into_stream()));
        });
        Arc::new(Ext4Processor::new(
            Arc::new(
                BlockDeviceReader::from_client_end(block_client_end)
                    .expect("failed to create block device reader"),
            ),
            read_only,
        ))
    }

    fn get_test_file_ino(processor: &Ext4Processor) -> u32 {
        processor.entry_at_path(Path::new("file1")).expect("failed entry at path").e2d_ino.get()
    }

    #[test_case(true; "ro")]
    #[test_case(false; "rw")]
    #[fuchsia::test]
    async fn test_read(read_only: bool) {
        let expected_content = b"file1 contents.\n";
        let processor = create_test_processor(read_only);
        let file_ino = get_test_file_ino(&processor);
        let file = ExtFile::from_processor(processor, file_ino as u32, read_only).unwrap();
        let proxy = serve_proxy(file, fio::PERM_READABLE);

        let content = proxy
            .read(expected_content.len() as u64)
            .await
            .expect("read FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("read error");
        assert_eq!(content.as_slice(), expected_content);

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");
    }

    #[test_case(true; "ro")]
    #[test_case(false; "rw")]
    #[fuchsia::test]
    async fn test_get_dac_attributes(read_only: bool) {
        let processor = create_test_processor(read_only);
        let file_ino = get_test_file_ino(&processor);
        let file = ExtFile::new(
            processor,
            file_ino.into(),
            ExtAttributes { mode: 0x8124, uid: 456, gid: 789 },
            XattrMap::default(),
            read_only,
        );
        let proxy = serve_proxy(file, fio::PERM_READABLE);

        let attributes_query = fio::NodeAttributesQuery::ID
            | fio::NodeAttributesQuery::MODE
            | fio::NodeAttributesQuery::UID
            | fio::NodeAttributesQuery::GID;
        let (mutable_attributes, immutable_attributes) = proxy
            .get_attributes(attributes_query)
            .await
            .expect("get_attributes FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("get_attributes error");
        assert_eq!(immutable_attributes.id.expect("missing id attribute"), u64::from(file_ino));
        assert_eq!(mutable_attributes.mode.expect("missing mode attribute"), 0x8124);
        assert_eq!(mutable_attributes.uid.expect("missing uid attribute"), 456);
        assert_eq!(mutable_attributes.gid.expect("missing gid attribute"), 789);

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");
    }

    #[test_case(true; "ro")]
    #[test_case(false; "rw")]
    #[fuchsia::test]
    async fn test_get_extended_attributes(read_only: bool) {
        let xattrs =
            [(b"attr".into(), b"value".into()), (b"attr2".into(), b"value2".into())].into();
        let processor = create_test_processor(read_only);
        let file_ino = get_test_file_ino(&processor);
        let file =
            ExtFile::new(processor, file_ino.into(), ExtAttributes::default(), xattrs, read_only);
        let proxy = serve_proxy(file, fio::PERM_READABLE);

        let value = proxy
            .get_extended_attribute(b"attr2")
            .await
            .expect("get_extended_attribute FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("get_extended_attribute error");
        assert_eq!(value, ExtendedAttributeValue::Bytes(b"value2".into()));

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");
    }

    #[fuchsia::test]
    async fn test_get_backing_memory() {
        let expected_content = b"file1 contents.\n";
        let processor = create_test_processor(true);
        let file_ino = get_test_file_ino(&processor);
        let file = ExtFile::from_processor(processor, file_ino as u32, true).unwrap();
        let proxy = serve_proxy(file, fio::PERM_READABLE);

        async fn assert_get_vmo(
            proxy: &fio::FileProxy,
            flags: fio::VmoFlags,
        ) -> Result<zx::Vmo, Status> {
            proxy
                .get_backing_memory(flags)
                .await
                .expect("get_backing_memory FIDL error")
                .map_err(zx::Status::err_from_raw)
        }

        fn assert_vmo_content(vmo: &zx::Vmo, expected: &[u8]) {
            let size = vmo.get_content_size().unwrap() as usize;
            assert_eq!(size, expected.len());
            let mut buffer = vec![0; size];
            vmo.read(&mut buffer, 0).unwrap();
            assert_eq!(buffer, expected);
        }

        let vmo = assert_get_vmo(&proxy, fio::VmoFlags::READ).await.unwrap();
        assert_vmo_content(&vmo, expected_content);

        let vmo = assert_get_vmo(&proxy, fio::VmoFlags::READ | fio::VmoFlags::SHARED_BUFFER)
            .await
            .unwrap();
        assert_vmo_content(&vmo, expected_content);

        let vmo = assert_get_vmo(&proxy, fio::VmoFlags::READ | fio::VmoFlags::PRIVATE_CLONE)
            .await
            .unwrap();
        assert_vmo_content(&vmo, expected_content);

        assert_eq!(
            assert_get_vmo(&proxy, fio::VmoFlags::READ | fio::VmoFlags::WRITE).await.unwrap_err(),
            Status::ACCESS_DENIED
        );
        assert_eq!(
            assert_get_vmo(
                &proxy,
                fio::VmoFlags::READ | fio::VmoFlags::WRITE | fio::VmoFlags::SHARED_BUFFER
            )
            .await
            .unwrap_err(),
            Status::ACCESS_DENIED
        );
        assert_eq!(
            assert_get_vmo(
                &proxy,
                fio::VmoFlags::READ | fio::VmoFlags::WRITE | fio::VmoFlags::PRIVATE_CLONE
            )
            .await
            .unwrap_err(),
            Status::ACCESS_DENIED
        );

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");
    }

    #[fuchsia::test]
    async fn test_rw_file() {
        // Create a device that is Ext4 formatted.
        let processor = create_test_processor(/* read_only=*/ false);
        let file_ino = get_test_file_ino(&processor);
        let ro_file =
            ExtFile::from_processor(processor.clone(), file_ino, /* read_only=*/ true)
                .expect("from_data error");
        let ro_proxy = serve_proxy(ro_file, fio::PERM_READABLE | fio::PERM_WRITABLE);
        ro_proxy.write(b"Write some stuff").await.expect_err("write FIDL request should fail");

        let rw_file = ExtFile::from_processor(processor, file_ino, /* read_only=*/ false)
            .expect("from_data error");
        let proxy = serve_proxy(rw_file, fio::PERM_READABLE | fio::PERM_WRITABLE);

        let expected_content = "file1 contents.\n";
        let content = proxy
            .read(expected_content.len() as u64)
            .await
            .expect("read FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("read error");
        assert_eq!(content.as_slice(), expected_content.as_bytes());

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");
    }

    #[test_case(true; "read only file")]
    #[test_case(false; "read write file")]
    #[fuchsia::test]
    async fn test_read_past_end_of_file(read_only: bool) {
        let processor = create_test_processor(read_only);
        let file_ino = get_test_file_ino(&processor);
        let file =
            ExtFile::from_processor(processor, file_ino, read_only).expect("from_data error");
        let proxy = serve_proxy(file, fio::PERM_READABLE);

        // Read from start past the end.
        let expected_content = "file1 contents.\n";
        let content = proxy
            .read(1024)
            .await
            .expect("read FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("read error");
        assert_eq!(content.as_slice(), expected_content.as_bytes());

        // Read from exactly at the end.
        let count = 10;
        let read_buf = proxy
            .read_at(count, content.len() as u64)
            .await
            .expect("read_at FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("read_at error");
        assert_eq!(read_buf.len(), 0);

        // Read from past the end.
        let read_buf = proxy
            .read_at(count, content.len() as u64 + 1)
            .await
            .expect("read_at FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("read_at error");
        assert_eq!(read_buf.len(), 0);

        // Read from the middle past the end.
        let offset = 7;
        let read_buf = proxy
            .read_at(count, offset)
            .await
            .expect("read_at FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("read_at error");
        assert_eq!(read_buf.len(), content.len() - offset as usize);

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");
    }

    #[fuchsia::test]
    async fn test_writing_past_eof_fails() {
        let read_only = false;
        let processor = create_test_processor(read_only);
        let file_ino = get_test_file_ino(&processor);

        let rw_file = ExtFile::from_processor(processor.clone(), file_ino, read_only)
            .expect("from_data error");
        let proxy = serve_proxy(rw_file, fio::PERM_READABLE | fio::PERM_WRITABLE);
        let original_content = proxy
            .read(1024)
            .await
            .expect("read FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("read error");

        // Append to the file (should still be within allocated region, but the implementation
        // doesn't support writing past EOF).
        let write_content = b"Write some stuff";
        let error = proxy
            .write(write_content)
            .await
            .expect("write FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect_err("write past EOF should fail");
        assert_eq!(error, Status::NOT_SUPPORTED);

        // Write a really long content, past allocated extents of this file.
        let write_content = [1u8; 8192];
        let error = proxy.write_at(&write_content, 0).await.expect("write FIDL error");
        assert_eq!(error, Err(zx::Status::NOT_SUPPORTED.into_raw()));

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");

        // Make sure no data was written
        let ro_file =
            ExtFile::from_processor(processor.clone(), file_ino, /* read_only=*/ true)
                .expect("from_data error");
        let ro_proxy = serve_proxy(ro_file, fio::PERM_READABLE);
        let verify_content = ro_proxy
            .read(1024)
            .await
            .expect("read FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("read error");
        assert_eq!(verify_content, original_content);
    }

    #[test_case(true; "ro")]
    #[test_case(false; "rw")]
    #[fuchsia::test]
    async fn test_lazy_loading_vmo_not_created_at_init(read_only: bool) {
        let processor = create_test_processor(read_only);
        let file_ino = get_test_file_ino(&processor);

        let file =
            ExtFile::from_processor(processor, file_ino, read_only).expect("from_processor failed");

        // Verify VMO is not loaded upon initial creation.
        assert!(!file.is_vmo_loaded(), "VMO should not be created at initialization");

        let proxy = serve_proxy(file.clone(), fio::PERM_READABLE);

        // Accessing file content triggers VMO loading.
        let _ = proxy
            .read(1024)
            .await
            .expect("read FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("read error");

        assert!(file.is_vmo_loaded(), "VMO should be loaded on demand after read");

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");
    }

    #[test_case(true; "ro")]
    #[test_case(false; "rw")]
    #[fuchsia::test]
    async fn test_vmo_discard(read_only: bool) {
        let processor = create_test_processor(read_only);
        let file_ino = get_test_file_ino(&processor);

        let file =
            ExtFile::from_processor(processor, file_ino, read_only).expect("from_processor failed");

        let proxy = serve_proxy(file.clone(), fio::PERM_READABLE);
        let _ = proxy
            .read(1024)
            .await
            .expect("read FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("read error");

        assert!(file.is_vmo_loaded(), "VMO should be loaded while proxy is active");

        proxy
            .close()
            .await
            .expect("close FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("close error");

        assert!(
            !file.is_vmo_loaded(),
            "VMO should be discarded when all connections to an unmodified file close"
        );
    }
}

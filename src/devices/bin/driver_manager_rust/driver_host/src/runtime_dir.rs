// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_lock::OnceCell;
use fidl_fuchsia_driver_host as fdh;
use fidl_fuchsia_io as fio;
use std::sync::Arc;
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::directory::simple::Simple;
use vfs::execution_scope::ExecutionScope;
use vfs::file::{FidlIoConnection, File, FileIo, FileLike, FileOptions, SyncMode, read_only};
use vfs::node::Node;
use vfs::{ObjectRequestRef, immutable_attributes, pseudo_directory};
use zx::Status;

#[derive(Clone)]
pub struct ProcessInfo {
    pub job_koid: zx::Koid,
    pub process_koid: zx::Koid,
    pub main_thread_koid: zx::Koid,
    pub threads: Vec<fdh::ThreadInfo>,
    pub dispatchers: Vec<fdh::DispatcherInfo>,
}

pub(crate) struct CachedProcessInfo {
    cell: OnceCell<ProcessInfo>,
    driver_host: fdh::DriverHostProxy,
}

impl CachedProcessInfo {
    pub(crate) fn new(driver_host: fdh::DriverHostProxy) -> Self {
        Self { cell: OnceCell::new(), driver_host }
    }

    pub(crate) async fn get(&self) -> Result<&ProcessInfo, zx::Status> {
        self.cell
            .get_or_try_init(|| async {
                match self.driver_host.get_process_info().await {
                    Ok(Ok((job_koid, process_koid, main_thread_koid, threads, dispatchers))) => {
                        Ok(ProcessInfo {
                            job_koid: zx::Koid::from_raw(job_koid),
                            process_koid: zx::Koid::from_raw(process_koid),
                            main_thread_koid: zx::Koid::from_raw(main_thread_koid),
                            threads,
                            dispatchers,
                        })
                    }
                    Ok(Err(e)) => Err(zx::Status::err_from_raw(e)),
                    Err(e) => {
                        log::error!("FIDL error GetProcessInfo: {:?}", e);
                        Err(zx::Status::INTERNAL)
                    }
                }
            })
            .await
    }
}

/// An implementation of `vfs::File` that reads its contents from the driver host's process info.
struct ElfFile {
    process_info: Arc<CachedProcessInfo>,
    info_extractor: fn(&ProcessInfo) -> String,
}

impl DirectoryEntry for ElfFile {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_file(self)
    }
}

impl GetEntryInfo for ElfFile {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)
    }
}

impl Node for ElfFile {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        let info = self.process_info.get().await?;
        let content = (self.info_extractor)(info);
        let content_size = content.len() as u64;
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::FILE,
                abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::READ_BYTES,
                content_size: content_size,
                storage_size: content_size,
            }
        ))
    }
}

impl FileIo for ElfFile {
    async fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<u64, Status> {
        let info = self.process_info.get().await?;
        let content = (self.info_extractor)(info);
        let bytes = content.as_bytes();
        let content_size = bytes.len() as u64;

        if offset >= content_size {
            return Ok(0u64);
        }

        let start = offset as usize;
        let read_len = std::cmp::min(bytes.len() - start, buffer.len());
        buffer[..read_len].copy_from_slice(&bytes[start..][..read_len]);
        Ok(read_len as u64)
    }

    async fn write_at(&self, _offset: u64, _content: &[u8]) -> Result<u64, Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn append(&self, _content: &[u8]) -> Result<(u64, u64), Status> {
        Err(Status::NOT_SUPPORTED)
    }
}

impl File for ElfFile {
    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn executable(&self) -> bool {
        false
    }

    async fn open_file(&self, _options: &FileOptions) -> Result<(), Status> {
        Ok(())
    }

    async fn truncate(&self, _length: u64) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn get_size(&self) -> Result<u64, Status> {
        let info = self.process_info.get().await?;
        let content = (self.info_extractor)(info);
        Ok(content.len() as u64)
    }

    async fn update_attributes(
        &self,
        _attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn sync(&self, _mode: SyncMode) -> Result<(), Status> {
        Ok(())
    }
}

impl FileLike for ElfFile {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        options: FileOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        FidlIoConnection::create_sync(scope, self, options, object_request.take());
        Ok(())
    }
}

/// Creates the runtime directory that is served to the driver host.
/// This directory contains information about the driver host process that can be used by debugging
/// tools like zxdb.
pub(crate) fn create_runtime_dir(process_info: Arc<CachedProcessInfo>) -> Arc<Simple> {
    let now = zx::MonotonicInstant::get().into_nanos().to_string();
    let process_start_time = read_only(now.into_bytes());

    let job_id = Arc::new(ElfFile {
        process_info: process_info.clone(),
        info_extractor: |info| info.job_koid.raw_koid().to_string(),
    });

    let process_id = Arc::new(ElfFile {
        process_info,
        info_extractor: |info| info.process_koid.raw_koid().to_string(),
    });

    pseudo_directory! {
        "elf" => pseudo_directory! {
            "process_start_time" => process_start_time,
            "job_id" => job_id,
            "process_id" => process_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{create_proxy, create_proxy_and_stream};
    use futures::stream::StreamExt;

    #[fuchsia::test]
    async fn test_runtime_dir_creation() {
        let (proxy, mut stream) = create_proxy_and_stream::<fdh::DriverHostMarker>();
        let process_info = Arc::new(CachedProcessInfo::new(proxy));

        // Mock the get_process_info call
        fuchsia_async::Task::spawn(async move {
            if let Some(Ok(fdh::DriverHostRequest::GetProcessInfo { responder })) =
                stream.next().await
            {
                responder.send(Ok((123, 456, 789, &[], &[]))).unwrap();
            }
        })
        .detach();

        let dir = create_runtime_dir(process_info);

        let scope = ExecutionScope::new();
        let (root_proxy, root_server) = create_proxy::<fio::DirectoryMarker>();
        vfs::directory::serve_on(dir, fio::PERM_READABLE, scope, root_server);

        let entries = fuchsia_fs::directory::readdir(&root_proxy).await.unwrap();
        // Check for "elf" directory
        assert!(entries.iter().any(|e| e.name == "elf"));

        let elf_proxy =
            fuchsia_fs::directory::open_directory(&root_proxy, "elf", fio::PERM_READABLE)
                .await
                .unwrap();
        let elf_entries = fuchsia_fs::directory::readdir(&elf_proxy).await.unwrap();
        assert!(elf_entries.iter().any(|e| e.name == "process_start_time"));
        assert!(elf_entries.iter().any(|e| e.name == "job_id"));
        assert!(elf_entries.iter().any(|e| e.name == "process_id"));

        let job_id_file =
            fuchsia_fs::directory::open_file(&elf_proxy, "job_id", fio::Flags::PERM_READ_BYTES)
                .await
                .unwrap();
        let job_id = fuchsia_fs::file::read_to_string(&job_id_file).await.unwrap();
        assert_eq!(job_id, "123");

        let process_id_file =
            fuchsia_fs::directory::open_file(&elf_proxy, "process_id", fio::Flags::PERM_READ_BYTES)
                .await
                .unwrap();
        let process_id = fuchsia_fs::file::read_to_string(&process_id_file).await.unwrap();
        assert_eq!(process_id, "456");
    }
}

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::backend::{BlockBackend, DeviceAttrs, Request, Sector};
use anyhow::{Error, anyhow};
use async_lock::Semaphore;
use async_trait::async_trait;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_io::{self as fio, FileMarker, FileProxy, MAX_BUF};
use fuchsia_trace as ftrace;
use futures::future::try_join_all;
use virtio_device::mem::DeviceRange;
use zx_status;

const MAX_INFLIGHT_REQUESTS: usize = 64;

pub struct FileBackend {
    file: FileProxy,
    // https://fxbug.dev/42076154: The guest can cause an unbounded number of requests to the backing file.
    // Due to the current lack of channel back-pressure this could result in a policy violation
    // and termination of the virtio-block process.
    //
    // Work around this by grabbing a ticket from the semaphore before sending any requests to the
    // file and holding it for the duration of the operation.
    semaphore: Semaphore,
}

impl FileBackend {
    pub fn new(file: ClientEnd<FileMarker>) -> Result<Self, Error> {
        let file = file.into_proxy();
        Ok(Self { file, semaphore: Semaphore::new(MAX_INFLIGHT_REQUESTS) })
    }

    /// Reads a single DeviceRange from the file.
    ///
    /// The caller must ensure that `DeviceRange` is no longer that `fidl_fuchsia_io::MAX_BUF`.
    async fn read_range<'a, 'b>(
        &self,
        offset: u64,
        range: DeviceRange<'a>,
        trace_id: ftrace::Id,
    ) -> Result<(), Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "FileBackend::read_range", "len" => range.len() as u64);
        assert!(range.len() <= MAX_BUF as usize);
        let bytes = {
            let _ticket = self.semaphore.acquire().await;
            self.file
                .read_at(range.len() as u64, offset)
                .await?
                .map_err(zx_status::Status::err_from_raw)?
        };
        if bytes.len() != range.len() {
            return Err(anyhow!(
                "Incorrect number of bytes read from file. Wanted {} got {}",
                range.len(),
                bytes.len()
            ));
        }
        // SAFETY: the range comes from the virtio chain and alignment is verified by
        // `try_mut_ptr`.
        let ptr = range.try_mut_ptr().unwrap();
        unsafe { libc::memmove(ptr, bytes.as_ptr() as *const libc::c_void, bytes.len()) };
        Ok(())
    }

    /// Writes a single DeviceRange to the file.
    ///
    /// The caller must ensure that `DeviceRange` is no longer that `fidl_fuchsia_io::MAX_BUF`.
    async fn write_range<'a, 'b>(
        &self,
        offset: u64,
        range: DeviceRange<'a>,
        trace_id: ftrace::Id,
    ) -> Result<(), Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "FileBackend::write_range", "len" => range.len() as u64);
        assert!(range.len() <= MAX_BUF as usize);
        // SAFETY: the range comes from the virtio chain and alignment is verified by `try_ptr`.
        let slice = unsafe { std::slice::from_raw_parts(range.try_ptr().unwrap(), range.len()) };
        let bytes_written = {
            let _ticket = self.semaphore.acquire().await;
            self.file.write_at(slice, offset).await?.map_err(zx_status::Status::err_from_raw)?
        };
        if bytes_written < range.len() as u64 {
            return Err(anyhow!(
                "Incorrect number of bytes read written to file. Wanted {} got {}",
                range.len(),
                bytes_written
            ));
        }
        Ok(())
    }
}

#[async_trait(?Send)]
impl BlockBackend for FileBackend {
    async fn get_attrs(&self, trace_id: ftrace::Id) -> Result<DeviceAttrs, Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "FileBackend::get_attrs");
        let (_, immutable_attributes) = self
            .file
            .get_attributes(fio::NodeAttributesQuery::CONTENT_SIZE)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        return Ok(DeviceAttrs {
            capacity: Sector::from_bytes_round_down(
                immutable_attributes.content_size.ok_or_else(|| anyhow!("content size not set"))?,
            ),
            block_size: None,
        });
    }

    async fn read<'a, 'b>(
        &self,
        request: Request<'a, 'b>,
        trace_id: ftrace::Id,
    ) -> Result<(), Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "FileBackend::read");
        try_join_all(
            request
                .ranges_bounded(MAX_BUF as usize)
                .map(|(offset, range)| self.read_range(offset, range, trace_id)),
        )
        .await?;
        Ok(())
    }

    async fn write<'a, 'b>(
        &self,
        request: Request<'a, 'b>,
        trace_id: ftrace::Id,
    ) -> Result<(), Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "FileBackend::write");
        try_join_all(
            request
                .ranges_bounded(MAX_BUF as usize)
                .map(|(offset, range)| self.write_range(offset, range, trace_id)),
        )
        .await?;
        Ok(())
    }

    async fn flush(&self, trace_id: ftrace::Id) -> Result<(), Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "FileBackend::flush");
        let _ticket = self.semaphore.acquire().await;
        self.file.sync().await?.map_err(zx_status::Status::err_from_raw)?;
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::backend_test::{BackendController, BackendTest};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::AsFd;
    use tempfile::tempfile;

    pub struct FileBackendController(std::fs::File);

    impl BackendController for FileBackendController {
        fn write_sector(&mut self, sector: Sector, data: &[u8]) -> Result<(), Error> {
            self.0.seek(SeekFrom::Start(sector.to_bytes().unwrap()))?;
            self.0.write_all(data)?;
            // Sync because we'll read this from the remote file so we want to make sure there's no
            // buffering in the way.
            self.0.sync_all()?;
            Ok(())
        }

        fn read_sector(&mut self, sector: Sector, data: &mut [u8]) -> Result<(), Error> {
            self.0.seek(SeekFrom::Start(sector.to_bytes().unwrap()))?;
            self.0.read_exact(data)?;
            Ok(())
        }
    }

    pub struct FileBackendTest;

    #[async_trait(?Send)]
    impl BackendTest for FileBackendTest {
        type Backend = FileBackend;
        type Controller = FileBackendController;

        async fn create_with_size(
            size: u64,
        ) -> Result<(FileBackend, FileBackendController), Error> {
            let file = tempfile()?;
            file.set_len(size)?;
            Ok((
                FileBackend::new(fdio::clone_channel(file.as_fd())?.into())?,
                FileBackendController(file),
            ))
        }
    }

    crate::backend_test::instantiate_backend_test_suite!(FileBackendTest);
}

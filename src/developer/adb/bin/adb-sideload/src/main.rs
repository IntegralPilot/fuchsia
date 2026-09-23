// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fidl_fuchsia_hardware_adb::{ProviderMarker, ProviderRequest, ProviderRequestStream};
use fidl_fuchsia_recovery_android::UpdaterMarker;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use fuchsia_fs::file::{AsyncReadAt, AsyncReadAtExt as _};
use futures::lock::Mutex;
use futures::{StreamExt as _, TryFutureExt as _};
use std::sync::Arc;

mod http_server;
mod sideload_file;

struct SideloadServer {}

impl SideloadServer {
    async fn connect_to_service(
        &self,
        socket: fidl::Socket,
        args: Option<String>,
    ) -> Result<(), zx::Status> {
        let [file_size, block_size] =
            *args.as_deref().unwrap_or_default().split(':').collect::<Vec<_>>()
        else {
            log::error!("Unexpected number of args: {args:?}");
            return Err(zx::Status::INVALID_ARGS);
        };
        let file_size = file_size.parse::<u64>().map_err(|e| {
            log::error!("Failed to parse file size from args {file_size:?}: {e}");
            zx::Status::INVALID_ARGS
        })?;
        let block_size = block_size.parse::<usize>().map_err(|e| {
            log::error!("Failed to parse block size from args {block_size:?}: {e}");
            zx::Status::INVALID_ARGS
        })?;
        if block_size == 0 {
            log::error!("Block size must be greater than 0");
            return Err(zx::Status::INVALID_ARGS);
        }
        log::info!("Starting sideload, file size: {file_size}, block size: {block_size}");
        fasync::Task::spawn(
            async move {
                let mut sideload_file = sideload_file::SideloadFile::new(
                    fasync::Socket::from_socket(socket),
                    file_size,
                    block_size,
                );
                let far_offset = find_far_offset(&mut sideload_file, file_size).await?;
                let far_file = sideload_file.into_sub_file(far_offset, file_size - far_offset);
                let archive = fuchsia_archive::AsyncUtf8Reader::new(Arc::new(Mutex::new(far_file)))
                    .await
                    .context("Failed to parse far")?;
                let (server_fut, close_fn, addr) = http_server::start_server(archive, block_size)
                    .context("Failed to start http server")?;
                let server_task = fasync::Task::spawn(server_fut);
                log::info!("http server listening on port {}", addr.port());
                let updater = connect_to_protocol::<UpdaterMarker>()
                    .context("Failed to connect to updater")?;
                updater.update(&format!("http://localhost:{}/ota_manifest", addr.port())).await?;
                log::info!("Shutting down http server");
                let far_reader = close_fn();
                let () = server_task.await.context("server task")?;
                let far_reader = Arc::into_inner(far_reader)
                    .ok_or_else(|| anyhow::anyhow!("Failed to unwrap Arc"))?;
                let far_file = far_reader.into_source();
                let far_file = Arc::into_inner(far_file)
                    .ok_or_else(|| anyhow::anyhow!("Failed to unwrap Arc"))?
                    .into_inner();
                let () = far_file.close().await.context("Failed to close sideload file")?;
                Ok(())
            }
            .unwrap_or_else(|e: Error| log::error!("Error in sideload server: {e:#}")),
        )
        .detach();
        Ok(())
    }
}

/// Find the offset of the first occurrence of the 8 bytes FAR magic header in the given file.
async fn find_far_offset(
    sideload_file: &mut (impl AsyncReadAt + Unpin),
    file_size: u64,
) -> Result<u64, Error> {
    use fuchsia_archive::MAGIC_INDEX_VALUE;

    let mut buf = [0; 4096];
    let mut offset = 0;
    loop {
        if offset + MAGIC_INDEX_VALUE.len() as u64 > file_size {
            anyhow::bail!("Failed to find far offset");
        }
        let read_len = (buf.len() as u64).min(file_size - offset) as usize;
        let bytes_read = sideload_file
            .read_at(offset, &mut buf[..read_len])
            .await
            .with_context(|| format!("Failed to read at offset {offset}"))?;
        if let Some(index) = memchr::memmem::find(&buf[..bytes_read], &MAGIC_INDEX_VALUE) {
            return Ok(offset + index as u64);
        }
        // Include the last few bytes in the next iteration, in case the magic is split across two
        // reads.
        offset += bytes_read.saturating_sub(MAGIC_INDEX_VALUE.len() - 1) as u64;
    }
}

#[async_trait::async_trait]
impl fidl_server::AsyncRequestHandler<ProviderMarker> for SideloadServer {
    async fn handle_request(&self, request: ProviderRequest) -> Result<(), Error> {
        let ProviderRequest::ConnectToService { socket, args, responder } = request;
        log::info!("Connecting to service with args: {:?}", args);
        responder
            .send(self.connect_to_service(socket, args).await.map_err(|s| s.into_raw()))
            .context("Failed to send response")?;
        Ok(())
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(|stream: ProviderRequestStream| {
        fidl_server::serve_async_detached(stream, SideloadServer {})
    });

    fs.take_and_serve_directory_handle()?;

    let () = fs.collect().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_fs::file::Adapter;
    use futures::io::Cursor;
    use test_case::test_case;

    #[test_case(0, 100; "at start")]
    #[test_case(50, 100; "in middle")]
    #[test_case(92, 100; "at end")]
    #[test_case(4090, 8192; "split across reads")]
    #[fuchsia::test]
    async fn test_find_far_offset_found(offset: usize, size: usize) {
        let mut data = vec![0; size];
        data[offset..offset + fuchsia_archive::MAGIC_INDEX_VALUE.len()]
            .copy_from_slice(&fuchsia_archive::MAGIC_INDEX_VALUE);
        let mut file = Adapter::new(Cursor::new(data));
        let found_offset = find_far_offset(&mut file, size as u64).await.unwrap();
        assert_eq!(found_offset, offset as u64);
    }

    #[test_case(100)]
    #[test_case(2)]
    #[test_case(0)]
    #[fuchsia::test]
    async fn test_find_far_offset_not_found(size: usize) {
        let mut file = Adapter::new(Cursor::new(vec![0; size]));
        let result = find_far_offset(&mut file, size as u64).await;
        assert!(result.is_err());
    }
}

// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::backend::{BlockBackend, DeviceAttrs, Request, Sector};
use anyhow::{Error, anyhow};
use async_trait::async_trait;
use block_client::{BlockClient, BufferSlice, MutableBufferSlice, RemoteBlockClient};
use fidl_fuchsia_storage_block::BlockMarker;
use fuchsia_trace as ftrace;
use futures::future::try_join_all;
use virtio_device::mem::DeviceRange;

/// RemoteBackend is a BlockBackend that fulfills requests by interfacing with a Fuchsia block
/// device (see /sdk/fidl/fuchsia.hardware.block.driver/block.fidl).
///
/// This involves sharing a VMO with the block device and then sending requests over a FIFO to
/// read/write sectors from that VMO. This implementation has the following limitations:
///
///   * Read/write operations must be block aligned. The linux driver seems to be fine with this
///     limitation, however there is nothing in the specification that precludes the driver from
///     using sub-sector sized descriptors in the chain.
///
///   * All read/writes are bounced through a VMO managed by the RemoteBlockClient. At the time of
///     writing this was implemented as a single shared VMO that allows a single request to be in-
///     flight at any given time.
pub struct RemoteBackend {
    block_client: RemoteBlockClient,
}

impl RemoteBackend {
    pub async fn new(client_end: fidl::endpoints::ClientEnd<BlockMarker>) -> Result<Self, Error> {
        let proxy = client_end.into_proxy();
        let block_client = RemoteBlockClient::new(proxy).await?;
        Ok(Self { block_client })
    }

    async fn read_range<'a, 'b>(
        &self,
        offset: u64,
        range: DeviceRange<'a>,
        trace_id: ftrace::Id,
    ) -> Result<(), Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "RemoteBackend::read_range", "len" => range.len() as u64);
        let buffer = self.build_mutable_buffer_slice(offset, &range)?;
        self.block_client.read_at(buffer, offset).await?;
        Ok(())
    }

    async fn write_range<'a, 'b>(
        &self,
        offset: u64,
        range: DeviceRange<'a>,
        trace_id: ftrace::Id,
    ) -> Result<(), Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "RemoteBackend::write_range", "len" => range.len() as u64);
        let buffer = self.build_buffer_slice(offset, &range)?;
        self.block_client.write_at(buffer, offset).await?;
        Ok(())
    }

    fn build_mutable_buffer_slice<'a>(
        &self,
        offset: u64,
        range: &DeviceRange<'a>,
    ) -> Result<MutableBufferSlice<'_>, Error> {
        self.check_range(offset, &range)?;
        let slice =
            unsafe { std::slice::from_raw_parts_mut(range.try_mut_ptr().unwrap(), range.len()) };
        Ok(MutableBufferSlice::Memory(slice))
    }

    fn build_buffer_slice<'a>(
        &self,
        offset: u64,
        range: &DeviceRange<'a>,
    ) -> Result<BufferSlice<'_>, Error> {
        self.check_range(offset, &range)?;
        let slice =
            unsafe { std::slice::from_raw_parts(range.try_mut_ptr().unwrap(), range.len()) };
        Ok(BufferSlice::Memory(slice))
    }

    fn check_range<'a>(&self, offset: u64, range: &DeviceRange<'a>) -> Result<(), Error> {
        if offset % self.block_client.block_size() as u64 != 0
            || range.len() % self.block_client.block_size() as usize != 0
        {
            return Err(anyhow!("RemoteBackend does not support block un-aligned requests"));
        }
        return Ok(());
    }
}

#[async_trait(?Send)]
impl BlockBackend for RemoteBackend {
    async fn get_attrs(&self, trace_id: ftrace::Id) -> Result<DeviceAttrs, Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "RemoteBackend::get_attrs");
        let block_size = self.block_client.block_size();
        let block_count = self.block_client.block_count();
        Ok(DeviceAttrs {
            capacity: Sector::from_bytes_round_down(
                block_count.checked_mul(block_size as u64).unwrap(),
            ),
            block_size: Some(block_size),
        })
    }

    async fn read<'a, 'b>(
        &self,
        request: Request<'a, 'b>,
        trace_id: ftrace::Id,
    ) -> Result<(), Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "RemoteBackend::read");
        let mut offset = request.sector.to_bytes().unwrap();
        try_join_all(request.ranges.iter().cloned().map(|range| {
            let len = range.len() as u64;
            let result = self.read_range(offset, range, trace_id);
            offset = offset.checked_add(len).unwrap();
            result
        }))
        .await?;
        Ok(())
    }

    async fn write<'a, 'b>(
        &self,
        request: Request<'a, 'b>,
        trace_id: ftrace::Id,
    ) -> Result<(), Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "RemoteBackend::write");
        let mut offset = request.sector.to_bytes().unwrap();
        try_join_all(request.ranges.iter().cloned().map(|range| {
            let len = range.len() as u64;
            let result = self.write_range(offset, range, trace_id);
            offset = offset.checked_add(len).unwrap();
            result
        }))
        .await?;
        Ok(())
    }

    async fn flush(&self, trace_id: ftrace::Id) -> Result<(), Error> {
        let _trace = ftrace::async_enter!(trace_id, "machina", "RemoteBackend::flush");
        Ok(self.block_client.flush().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend_test::{BackendController, BackendTest};

    struct RemoteBackendController {
        vmo: zx::Vmo,
        // This is unused directly, but we need to hold on to it to keep the ramdisk around.
        _ramdisk_client: ramdevice_client::RamdiskClient,
    }

    impl RemoteBackendController {
        pub fn new(ramdisk_client: ramdevice_client::RamdiskClient, vmo: zx::Vmo) -> Self {
            Self { _ramdisk_client: ramdisk_client, vmo }
        }
    }

    impl BackendController for RemoteBackendController {
        fn write_sector(&mut self, sector: Sector, data: &[u8]) -> Result<(), Error> {
            self.vmo.write(data, sector.to_bytes().unwrap())?;
            Ok(())
        }

        fn read_sector(&mut self, sector: Sector, data: &mut [u8]) -> Result<(), Error> {
            self.vmo.read(data, sector.to_bytes().unwrap())?;
            Ok(())
        }
    }

    struct RemoteBackendTest<const BLOCK_SIZE: u64>;

    #[async_trait(?Send)]
    impl<const BLOCK_SIZE: u64> BackendTest for RemoteBackendTest<BLOCK_SIZE> {
        type Backend = RemoteBackend;
        type Controller = RemoteBackendController;

        async fn create_with_size(
            size: u64,
        ) -> Result<(RemoteBackend, RemoteBackendController), Error> {
            // Create a VMO to use for the ramdisk. The controller will interface with the VMO
            // directly to verify what has been written to the disk.
            let vmo = zx::Vmo::create(size).unwrap();
            let ramdisk_client = ramdevice_client::RamdiskClientBuilder::new_with_vmo(
                vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                Some(BLOCK_SIZE),
            )
            .build()
            .await
            .unwrap();

            // Open a connection to the ramdisk and use it to create the RemoteBackend.
            let channel = ramdisk_client.open().unwrap();
            let backend = RemoteBackend::new(channel).await.unwrap();
            let controller = RemoteBackendController::new(ramdisk_client, vmo);
            Ok((backend, controller))
        }
    }

    // The following tests currently do not pass with this backend:
    //
    //      test_read_subsector_range,
    //      test_write_subsector_range,
    //
    // This is because the block FIFO protocol can only support block-aligned transfers. To be
    // robust against these types of requests, we will need to add some logic to use some
    // temporaray buffers to handle transfers to those unaligned blocks.
    crate::backend_test::instantiate_backend_test_suite!(
        RemoteBackendTest<512>,
        test_get_attrs,
        test_read_per_sector_ranges,
        test_read_multiple_sectors_per_range,
        test_read_large,
        test_read_concurrent,
        test_write_per_sector_ranges,
        test_write_multiple_sectors_per_range,
        test_write_large,
        test_write_concurrent,
        test_read_write_loop
    );
}

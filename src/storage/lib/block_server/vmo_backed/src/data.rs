// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use block_server::async_interface::Interface;
use block_server::{DeviceInfo, ReadOptions, WriteOptions};
use std::borrow::Cow;
use std::num::NonZero;
use std::sync::Arc;

pub struct Data {
    pub info: DeviceInfo,
    pub block_size: u32,
    pub data: zx::Vmo,
}

impl Interface for Data {
    fn get_info(&self) -> Cow<'_, DeviceInfo> {
        Cow::Borrowed(&self.info)
    }

    async fn read(
        &self,
        device_block_offset: u64,
        block_count: u32,
        vmo: &Arc<zx::Vmo>,
        vmo_offset: u64,
        opts: ReadOptions,
        _trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        if opts.inline_crypto.is_enabled {
            return Err(zx::Status::IO);
        }

        if let Some(max) = self.info.max_transfer_blocks().as_ref() {
            // Requests should be split up by the core library
            assert!(block_count <= max.get());
        }
        if device_block_offset + block_count as u64 > self.info.block_count().unwrap() {
            Err(zx::Status::OUT_OF_RANGE)
        } else {
            let data = self.data.read_to_vec(
                device_block_offset * self.block_size as u64,
                block_count as u64 * self.block_size as u64,
            )?;
            vmo.write(&data[..], vmo_offset)
        }
    }

    async fn write(
        &self,
        device_block_offset: u64,
        block_count: u32,
        vmo: &Arc<zx::Vmo>,
        vmo_offset: u64,
        opts: WriteOptions,
        _trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        if opts.inline_crypto.is_enabled {
            return Err(zx::Status::IO);
        }
        if let Some(max) = self.info.max_transfer_blocks().as_ref() {
            // Requests should be split up by the core library
            assert!(block_count <= max.get());
        }
        if device_block_offset + block_count as u64 > self.info.block_count().unwrap() {
            Err(zx::Status::OUT_OF_RANGE)
        } else {
            let data = vmo.read_to_vec(vmo_offset, block_count as u64 * self.block_size as u64)?;
            self.data.write(&data[..], device_block_offset * self.block_size as u64)
        }
    }

    async fn flush(&self, _trace_flow_id: Option<NonZero<u64>>) -> Result<(), zx::Status> {
        Ok(())
    }

    async fn trim(
        &self,
        device_block_offset: u64,
        block_count: u32,
        _trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        if device_block_offset + block_count as u64 > self.info.block_count().unwrap() {
            Err(zx::Status::OUT_OF_RANGE)
        } else {
            Ok(())
        }
    }
}

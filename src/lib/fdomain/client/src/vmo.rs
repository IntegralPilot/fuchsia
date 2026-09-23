// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::handle::handle_type;
use crate::responder::Responder;
use crate::{Error, Handle, ordinals};
use fidl_fuchsia_fdomain as proto;
use futures::FutureExt;
use std::future::Future;

/// A virtual memory object (VMO) in a remote FDomain.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Vmo(pub(crate) Handle);

handle_type!(Vmo VMO);

impl Vmo {
    /// Read `size` bytes from the VMO at `offset`.
    pub fn read(
        &self,
        offset: u64,
        size: u64,
    ) -> impl Future<Output = Result<Vec<u8>, Error>> + use<> {
        let client = self.0.client();
        let handle = self.0.proto();

        client
            .transaction(
                ordinals::READ_VMO,
                proto::VmoReadVmoRequest { handle, offset, size },
                Responder::ReadVmo,
            )
            .map(|res| res.map(|r| r.data))
    }

    /// Read from the VMO into a buffer. Returns Ok(()) on success.
    pub async fn read_slice(&self, buf: &mut [u8], offset: u64) -> Result<(), Error> {
        let data = self.read(offset, buf.len() as u64).await?;
        if data.len() != buf.len() {
            return Err(Error::FDomain(proto::Error::TargetError(
                zx_status::Status::OUT_OF_RANGE.into_raw(),
            )));
        }
        buf.copy_from_slice(&data);
        Ok(())
    }

    /// Write all data to the VMO at `offset`.
    pub fn write(
        &self,
        data: &[u8],
        offset: u64,
    ) -> impl Future<Output = Result<(), Error>> + use<> {
        let client = self.0.client();
        let handle = self.0.proto();
        let data = data.to_vec();

        client.transaction(
            ordinals::WRITE_VMO,
            proto::VmoWriteVmoRequest { handle, offset, data },
            Responder::WriteVmo,
        )
    }

    /// Get the size of the VMO.
    pub fn get_size(&self) -> impl Future<Output = Result<u64, Error>> + use<> {
        let client = self.0.client();
        let handle = self.0.proto();

        client
            .transaction(
                ordinals::GET_VMO_SIZE,
                proto::VmoGetVmoSizeRequest { handle },
                Responder::GetVmoSize,
            )
            .map(|res| res.map(|r| r.size))
    }

    /// Set the size of the VMO.
    pub fn set_size(&self, size: u64) -> impl Future<Output = Result<(), Error>> + use<> {
        let client = self.0.client();
        let handle = self.0.proto();

        client.transaction(
            ordinals::SET_VMO_SIZE,
            proto::VmoSetVmoSizeRequest { handle, size },
            Responder::SetVmoSize,
        )
    }

    /// Get the stream size of the VMO.
    pub fn get_stream_size(&self) -> impl Future<Output = Result<u64, Error>> + use<> {
        let client = self.0.client();
        let handle = self.0.proto();

        client
            .transaction(
                ordinals::GET_VMO_STREAM_SIZE,
                proto::VmoGetVmoStreamSizeRequest { handle },
                Responder::GetVmoStreamSize,
            )
            .map(|res| res.map(|r| r.size))
    }

    /// Set the stream size of the VMO.
    pub fn set_stream_size(&self, size: u64) -> impl Future<Output = Result<(), Error>> + use<> {
        let client = self.0.client();
        let handle = self.0.proto();

        client.transaction(
            ordinals::SET_VMO_STREAM_SIZE,
            proto::VmoSetVmoStreamSizeRequest { handle, size },
            Responder::SetVmoStreamSize,
        )
    }
}

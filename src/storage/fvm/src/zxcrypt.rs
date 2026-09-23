// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Implements Zxcrypt.  This is tested via the fshost tests.

use super::{AlignedMem, Fvm, IoTrait, ReadToMem, WriteFromMem};
use crate::buffers::{BUFFER_SIZE, BufferGuard};
use crate::device::Device;
use aes::Aes256;
use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use anyhow::{Error, ensure};
use block_client::{BlockClient, BufferSlice, MutableBufferSlice, ReadOptions, WriteOptions};
use std::sync::Arc;

use futures::stream::{FuturesUnordered, TryStreamExt};
use storage_xts::{Tweak, XtsProcessor};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, little_endian};

pub struct Key {
    data_cipher: Aes256,
    iv_cipher: Aes256,
    iv: u128,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, FromBytes, Immutable, KnownLayout)]
struct ZxcryptHeaderAndKey {
    magic: u128,
    guid: [u8; 16],
    version: u32,

    // The header is immediately followed by keys a.k.a. "slots".  Only the first slot is used.

    // The wrapped key is 128 bit GCM SIV wrapping a 64-byte AES 256 XTS key and a 16 byte
    // IV.
    wrapped_key: [u8; 96],
}

const ZXCRYPT_MAGIC: u128 = 0x74707972_63787a80_e7116db3_00f8e85f;
const ZXCRYPT_VERSION: u32 = 0x01000000;

impl Key {
    pub async fn unseal(
        fvm: &Fvm,
        partition_index: u16,
        crypt: &fidl_fuchsia_fxfs::CryptProxy,
    ) -> Result<Self, Error> {
        // Read the first block which contains Zxcrypt's header.
        let block_size = fvm.device.block_size() as usize;
        let mut data = AlignedMem::<ZxcryptHeaderAndKey>::new(block_size);
        fvm.do_io(ReadToMem::new(&fvm.device, &mut data), partition_index, 0, 1, 0).await?;
        let (zxcrypt_header, _) = ZxcryptHeaderAndKey::ref_from_prefix(&data).unwrap();

        ensure!(zxcrypt_header.magic == ZXCRYPT_MAGIC, zx::Status::WRONG_TYPE);
        ensure!(zxcrypt_header.version == ZXCRYPT_VERSION, zx::Status::NOT_SUPPORTED);

        // This is tightly coupled with the implementation of Crypt in //src/storage/crypt/zxcrypt.
        // It expects to receive the zxcrypt header (which includes the magic, guid and version)
        // followed by the wrapped key.  The unwrapped key consists of 64 bytes for the XTS key
        // which is made up of two 32 bytes Aes256 keys, one for the data and one for the IV/tweak,
        // followed by 16 bytes which make up the IV.
        let key = fidl_fuchsia_fxfs::WrappedKey::Zxcrypt(
            data[..std::mem::size_of::<ZxcryptHeaderAndKey>()].to_vec(),
        );
        let unwrapped_key = crypt.unwrap_key(0, &key).await?.map_err(zx::Status::err_from_raw)?;

        Ok(Self {
            data_cipher: Aes256::new_from_slice(&unwrapped_key[..32]).unwrap(),
            iv_cipher: Aes256::new_from_slice(&unwrapped_key[32..64]).unwrap(),
            iv: little_endian::U128::from_bytes(unwrapped_key[64..80].try_into().unwrap()).get(),
        })
    }

    pub async fn format(
        fvm: &Fvm,
        partition_index: u16,
        crypt: &fidl_fuchsia_fxfs::CryptProxy,
    ) -> Result<Self, Error> {
        let block_size = fvm.device.block_size() as usize;
        let mut data = AlignedMem::<ZxcryptHeaderAndKey>::new(block_size);

        // Fill the block with random data.  This is what the old driver did.
        zx::cprng_draw(&mut data);

        let (_, key, unwrapped_key) = crypt
            .create_key(0, fidl_fuchsia_fxfs::KeyPurpose::Data)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        ensure!(key.len() == std::mem::size_of::<ZxcryptHeaderAndKey>(), zx::Status::INTERNAL);

        data[..std::mem::size_of::<ZxcryptHeaderAndKey>()].copy_from_slice(&key);

        let (zxcrypt_header, _) = ZxcryptHeaderAndKey::ref_from_prefix(&data).unwrap();
        ensure!(zxcrypt_header.magic == ZXCRYPT_MAGIC, zx::Status::INTERNAL);

        // Make sure the first two slices are allocated.
        fvm.ensure_allocated(partition_index, 2).await?;
        fvm.do_io(WriteFromMem::new(&fvm.device, &data), partition_index, 0, 1, 0).await?;

        Ok(Self {
            data_cipher: Aes256::new_from_slice(&unwrapped_key[..32]).unwrap(),
            iv_cipher: Aes256::new_from_slice(&unwrapped_key[32..64]).unwrap(),
            iv: little_endian::U128::from_bytes(unwrapped_key[64..80].try_into().unwrap()).get(),
        })
    }

    pub async fn shred_if_zxcrypt_volume(fvm: &Fvm, partition_index: u16) -> Result<(), Error> {
        // Read the first block which contains Zxcrypt's header.
        let block_size = fvm.device.block_size() as usize;
        let mut data = AlignedMem::<ZxcryptHeaderAndKey>::new(block_size);
        fvm.do_io(ReadToMem::new(&fvm.device, &mut data), partition_index, 0, 1, 0).await?;

        let (zxcrypt_header, _) = ZxcryptHeaderAndKey::ref_from_prefix(&data).unwrap();
        if zxcrypt_header.magic != ZXCRYPT_MAGIC {
            return Ok(());
        }

        // Fill the block with random data to shred it.  This is what the old driver did.
        zx::cprng_draw(&mut data);
        fvm.do_io(WriteFromMem::new(&fvm.device, &data), partition_index, 0, 1, 0).await?;

        Ok(())
    }
}

struct Op {
    offset: u64,
    len: u64,
    trace_flow_id: u64,
}

pub struct EncryptedRead<'a> {
    device: &'a Device,
    key: &'a Key,
    tweak: u128,
    vmo: Arc<zx::Vmo>,
    vmo_offset: u64,
    buffer: BufferGuard,
    private_buffer: BufferGuard,
    ops: Vec<Op>,
    queued_len: u64,
}

impl<'a> EncryptedRead<'a> {
    pub async fn new(
        device: &'a Device,
        key: &'a Key,
        block_offset: u64,
        vmo: Arc<zx::Vmo>,
        vmo_offset: u64,
    ) -> Self {
        Self {
            device,
            key,
            tweak: key.iv + block_offset as u128,
            vmo,
            vmo_offset,
            buffer: device.get_buffer().await,
            private_buffer: device.get_private_buffer().await,
            ops: Vec::new(),
            queued_len: 0,
        }
    }
}

impl IoTrait for EncryptedRead<'_> {
    async fn add_op(
        &mut self,
        mut offset: u64,
        mut len: u64,
        trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        loop {
            let space = BUFFER_SIZE as u64 - self.queued_len;
            if space >= len {
                break;
            }
            if space > 0 {
                self.ops.push(Op { offset, len: space, trace_flow_id });
            }
            self.queued_len += space;
            self.flush().await?;
            offset += space;
            len -= space;
        }
        self.ops.push(Op { offset, len, trace_flow_id });
        self.queued_len += len;
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), zx::Status> {
        // Read into the buffer.
        let mut buf_offset = 0;
        let futures = FuturesUnordered::from_iter(self.ops.drain(..).map(
            |Op { offset, len, trace_flow_id }| {
                let fut = self.device.read_at_with_opts_traced(
                    MutableBufferSlice::new_with_vmo_id(
                        self.device.shared_vmo_id(),
                        self.buffer.vmo_offset() + buf_offset,
                        len,
                    ),
                    offset,
                    ReadOptions::default(),
                    trace_flow_id,
                );
                buf_offset += len;
                fut
            },
        ));
        self.queued_len = 0;
        let () = futures.try_collect().await?;

        let src_slice = self.buffer.as_ptr_slice().subslice(0..buf_offset as usize);
        let mut dst_slice =
            self.private_buffer.as_mut_ptr_slice().subslice_mut(0..buf_offset as usize);

        // Decrypt the buffer
        let iv = &mut self.tweak;
        let block_size = self.device.block_size() as usize;
        assert_eq!(buf_offset as usize % block_size, 0, "buf_offset must be block aligned");
        let mut sector_offset = 0;
        while sector_offset < buf_offset as usize {
            let src_sector = src_slice.subslice(sector_offset..sector_offset + block_size);
            let dst_sector = dst_slice.subslice_mut(sector_offset..sector_offset + block_size);
            let mut tweak = Tweak(*iv);
            self.key.iv_cipher.encrypt_block(tweak.as_mut_bytes().try_into().unwrap());
            self.key
                .data_cipher
                .decrypt_with_backend(XtsProcessor::new(tweak, src_sector, dst_sector));
            *iv += 1;
            sector_offset += block_size;
        }

        let ptr = dst_slice.as_mut_ptr();

        // SAFETY: self.private_buffer is private, safe to create &[u8]
        let slice = unsafe { std::slice::from_raw_parts(ptr, buf_offset as usize) };

        self.vmo.write(slice, self.vmo_offset)?;

        self.vmo_offset += buf_offset;
        Ok(())
    }
}

pub struct EncryptedWrite<'a> {
    device: &'a Device,
    key: &'a Key,
    tweak: u128,
    vmo: Arc<zx::Vmo>,
    vmo_offset: u64,
    options: WriteOptions,
    buffer: BufferGuard,
    private_buffer: BufferGuard,
    ops: Vec<Op>,
    queued_len: u64,
}

impl<'a> EncryptedWrite<'a> {
    pub async fn new(
        device: &'a Device,
        key: &'a Key,
        block_offset: u64,
        vmo: Arc<zx::Vmo>,
        vmo_offset: u64,
        options: WriteOptions,
    ) -> Self {
        Self {
            device,
            key,
            tweak: key.iv + block_offset as u128,
            vmo,
            vmo_offset,
            options,
            buffer: device.get_buffer().await,
            private_buffer: device.get_private_buffer().await,
            ops: Vec::new(),
            queued_len: 0,
        }
    }
}

impl IoTrait for EncryptedWrite<'_> {
    async fn add_op(
        &mut self,
        mut offset: u64,
        mut len: u64,
        trace_flow_id: u64,
    ) -> Result<(), zx::Status> {
        loop {
            let space = BUFFER_SIZE as u64 - self.queued_len;
            if space >= len {
                break;
            }
            if space > 0 {
                self.ops.push(Op { offset, len: space, trace_flow_id });
            }
            self.queued_len += space;
            self.flush().await?;
            offset += space;
            len -= space;
        }
        self.ops.push(Op { offset, len, trace_flow_id });
        self.queued_len += len;
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), zx::Status> {
        let dst_slice =
            self.private_buffer.as_mut_ptr_slice().subslice_mut(0..self.queued_len as usize);
        let ptr = dst_slice.as_mut_ptr();

        // SAFETY: self.private_buffer is private, and we have &mut self, so it is safe to create
        // &mut [u8]
        let slice_mut = unsafe { std::slice::from_raw_parts_mut(ptr, self.queued_len as usize) };

        self.vmo.read(slice_mut, self.vmo_offset)?;
        self.vmo_offset += self.queued_len;

        let src_slice = dst_slice.as_ptr_slice();
        let mut encrypt_dst_slice =
            self.buffer.as_mut_ptr_slice().subslice_mut(0..self.queued_len as usize);

        // Encrypt the buffer
        let iv = &mut self.tweak;
        let block_size = self.device.block_size() as usize;
        assert_eq!(self.queued_len as usize % block_size, 0, "queued_len must be block aligned");
        let mut sector_offset = 0;
        while sector_offset < self.queued_len as usize {
            let src_sector = src_slice.subslice(sector_offset..sector_offset + block_size);
            let dst_sector =
                encrypt_dst_slice.subslice_mut(sector_offset..sector_offset + block_size);
            let mut tweak = Tweak(*iv);
            self.key.iv_cipher.encrypt_block(tweak.as_mut_bytes().try_into().unwrap());
            self.key
                .data_cipher
                .encrypt_with_backend(XtsProcessor::new(tweak, src_sector, dst_sector));
            *iv += 1;
            sector_offset += block_size;
        }
        self.queued_len = 0;

        // Write to the device.
        let mut buf_offset = 0;
        FuturesUnordered::from_iter(self.ops.drain(..).map(|Op { offset, len, trace_flow_id }| {
            let fut = self.device.write_at_with_opts_traced(
                BufferSlice::new_with_vmo_id(
                    self.device.shared_vmo_id(),
                    self.buffer.vmo_offset() + buf_offset,
                    len,
                ),
                offset,
                self.options,
                trace_flow_id,
            );
            buf_offset += len;
            fut
        }))
        .try_collect()
        .await
    }
}

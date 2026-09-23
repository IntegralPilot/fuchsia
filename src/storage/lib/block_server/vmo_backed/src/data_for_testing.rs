// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use block_server::async_interface::Interface;
use block_server::{DeviceInfo, ReadOptions, WriteFlags, WriteOptions};
use fuchsia_sync::{Mutex, MutexGuard};
use fxfs_crypto::{FscryptSoftwareInoLblk32FileCipher, UnwrappedKey};
use rand::RngExt as _;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

pub struct Data {
    pub info: DeviceInfo,
    pub block_size: u32,
    pub data: zx::Vmo,
    pub write_cache: Option<Mutex<WriteCache>>,
    pub max_jitter_usec: Option<u64>,
    pub observer: Option<Box<dyn Observer>>,
    // Maps keyslots to lblk32 software ciphers used to encrypt/decrypt file contents.
    pub fscrypt_keys: Mutex<FscryptKeys>,
}

impl Data {
    async fn add_jitter(&self) {
        if let Some(max) = self.max_jitter_usec {
            fuchsia_async::Timer::new(Duration::from_micros(rand::random_range(0..max))).await
        }
    }

    fn write_cache(&self) -> Option<MutexGuard<'_, WriteCache>> {
        self.write_cache.as_ref().map(Mutex::lock)
    }

    pub fn client_closed(&self) -> Result<(), zx::Status> {
        if let Some(mut cache) = self.write_cache() {
            if let Some(observer) = self.observer.as_ref() {
                observer.close(Some(&mut *cache));
            }
            cache.apply(&self.data)
        } else {
            if let Some(observer) = self.observer.as_ref() {
                observer.close(None);
            }
            Ok(())
        }
    }

    pub fn fscrypt_keys(&self) -> MutexGuard<'_, FscryptKeys> {
        self.fscrypt_keys.lock()
    }
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
        self.add_jitter().await;
        if let Some(observer) = self.observer.as_ref() {
            observer.read(device_block_offset, block_count, vmo, vmo_offset);
        }

        if let Some(max) = self.info.max_transfer_blocks().as_ref() {
            // Requests should be split up by the core library
            assert!(block_count <= max.get());
        }
        if device_block_offset + block_count as u64 > self.info.block_count().unwrap() {
            Err(zx::Status::OUT_OF_RANGE)
        } else {
            let mut data = if let Some(cache) = self.write_cache() {
                let mut data = vec![0u8; block_count as usize * self.block_size as usize];
                cache.read(&self.data, device_block_offset, &mut data[..])?;
                data
            } else {
                self.data.read_to_vec(
                    device_block_offset * self.block_size as u64,
                    block_count as u64 * self.block_size as u64,
                )?
            };
            if opts.inline_crypto.is_enabled {
                self.fscrypt_keys()
                    .get_key(opts.inline_crypto.slot)?
                    .decrypt(&mut data[..], opts.inline_crypto.dun as u128)
                    .map_err(|_| zx::Status::IO)?;
            }
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
        self.add_jitter().await;
        if let Some(observer) = self.observer.as_ref() {
            match observer.write(device_block_offset, block_count, vmo, vmo_offset, opts) {
                WriteAction::Write => {}
                WriteAction::Discard => return Ok(()),
                WriteAction::Fail => return Err(zx::Status::IO),
            }
        }

        if opts.flags.contains(WriteFlags::PRE_BARRIER) {
            if let Some(mut cache) = self.write_cache() {
                cache.apply(&self.data)?;
            }
        }
        if let Some(max) = self.info.max_transfer_blocks().as_ref() {
            // Requests should be split up by the core library
            assert!(block_count <= max.get());
        }
        if device_block_offset + block_count as u64 > self.info.block_count().unwrap() {
            Err(zx::Status::OUT_OF_RANGE)
        } else {
            let mut data =
                vmo.read_to_vec(vmo_offset, block_count as u64 * self.block_size as u64)?;
            if !opts.flags.contains(WriteFlags::FORCE_ACCESS)
                && let Some(mut cache) = self.write_cache()
            {
                cache.insert(device_block_offset, &data[..]);
            }
            if opts.inline_crypto.is_enabled {
                self.fscrypt_keys()
                    .get_key(opts.inline_crypto.slot)?
                    .encrypt(&mut data[..], opts.inline_crypto.dun as u128)
                    .map_err(|_| zx::Status::IO)?;
            }
            self.data.write(&data[..], device_block_offset * self.block_size as u64)
        }
    }

    async fn flush(&self, _trace_flow_id: Option<NonZero<u64>>) -> Result<(), zx::Status> {
        self.add_jitter().await;

        let mut cache = self.write_cache();
        if let Some(observer) = self.observer.as_ref() {
            match cache.as_mut() {
                Some(w) => observer.flush(Some(&mut *w)),
                None => observer.flush(None),
            }
        }
        if let Some(w) = cache.as_mut() { w.apply(&self.data) } else { Ok(()) }
    }

    async fn trim(
        &self,
        device_block_offset: u64,
        block_count: u32,
        _trace_flow_id: Option<NonZero<u64>>,
    ) -> Result<(), zx::Status> {
        self.add_jitter().await;

        if let Some(observer) = self.observer.as_ref() {
            observer.trim(device_block_offset, block_count);
        }
        if device_block_offset + block_count as u64 > self.info.block_count().unwrap() {
            Err(zx::Status::OUT_OF_RANGE)
        } else {
            Ok(())
        }
    }
}

/// Keeps track of a sequence of writes since the last flush or barrier, and allows them to be
/// arbitrarily modified or re-ordered.
pub struct WriteCache {
    block_size: u64,
    block_offsets: Vec<u64>,
    buffer: Vec<u8>,
}

impl WriteCache {
    pub(crate) fn new(block_size: u64) -> Self {
        Self { block_size, block_offsets: vec![], buffer: vec![] }
    }

    fn insert(&mut self, block_offset: u64, contents: &[u8]) {
        let block_count = contents.len() as u64 / self.block_size;
        let mut buf_offset = 0;
        for offset in block_offset..block_offset + block_count {
            self.block_offsets.push(offset);
            self.buffer
                .extend_from_slice(&contents[buf_offset..buf_offset + self.block_size as usize]);
            buf_offset += self.block_size as usize;
        }
    }

    // Reads the last written value, falling back to `data` if there are no local updates.
    fn read(
        &self,
        data: &zx::Vmo,
        block_offset: u64,
        contents: &mut [u8],
    ) -> Result<(), zx::Status> {
        let block_count = contents.len() as u64 / self.block_size;
        let max_offset = block_offset + block_count;
        data.read(contents, block_offset * self.block_size)?;
        // Apply any buffered writes that would overwrite the actual contents.  If the same offset
        // shows up multiple times, we want to use the most recent write, so it's important to
        // iterate in order.
        for (idx, offset) in self.block_offsets.iter().enumerate() {
            if *offset >= block_offset && *offset < max_offset {
                let in_offset = idx * self.block_size as usize;
                let out_offset = ((*offset - block_offset) * self.block_size) as usize;
                contents[out_offset..out_offset + self.block_size as usize]
                    .copy_from_slice(&self.buffer[in_offset..in_offset + self.block_size as usize]);
            }
        }
        Ok(())
    }

    // Persists all writes to `data` and empties the cache.
    fn apply(&mut self, data: &zx::Vmo) -> Result<(), zx::Status> {
        let mut buf_offset = 0;
        for offset in self.block_offsets.drain(..) {
            data.write(
                &self.buffer[buf_offset..buf_offset + self.block_size as usize],
                offset * self.block_size,
            )?;
            buf_offset += self.block_size as usize;
        }
        self.buffer.clear();
        Ok(())
    }

    /// Returns the number of writes in the batch.
    pub fn len(&self) -> usize {
        self.block_offsets.len()
    }

    /// Returns an iterator over the batch of writes (in temporal sequence).
    pub fn iter(&self) -> impl Iterator<Item = (&u64, &[u8])> {
        self.block_offsets.iter().zip(self.buffer.windows(self.block_size as usize))
    }

    fn swap_writes(&mut self, i: usize, j: usize) {
        self.block_offsets.swap(i, j);
        let bs = self.block_size as usize;
        let mut buf = vec![0u8; bs];
        buf.copy_from_slice(&self.buffer[i * bs..(i + 1) * bs]);
        self.buffer.copy_within(j * bs..(j + 1) * bs, i * bs);
        self.buffer[j * bs..(j + 1) * bs].copy_from_slice(&buf[..]);
    }

    /// Reorders all writes.
    pub fn shuffle(&mut self) {
        // Implements the Fisher-Yates shuffle.
        let mut rng = rand::rng();
        for i in 0..self.block_offsets.len() {
            let j = rng.random_range(0..=i);
            if i != j {
                self.swap_writes(i, j);
            }
        }
    }

    /// Discards a random number of writes from the tail, simulating a power-cut.
    pub fn discard_some(&mut self) {
        let mut rng = rand::rng();
        let idx = rng.random_range(0..=self.block_offsets.len());
        for i in idx..self.block_offsets.len() {
            self.buffer[i * self.block_size as usize..(i + 1) * self.block_size as usize]
                .fill(0xab);
        }
    }
}

/// The Observer can silently discard writes, or fail them explicitly (zx::Status::IO is returned).
pub enum WriteAction {
    Write,
    Discard,
    Fail,
}

pub trait Observer: Send + Sync {
    fn read(
        &self,
        _device_block_offset: u64,
        _block_count: u32,
        _vmo: &Arc<zx::Vmo>,
        _vmo_offset: u64,
    ) {
    }

    fn write(
        &self,
        _device_block_offset: u64,
        _block_count: u32,
        _vmo: &Arc<zx::Vmo>,
        _vmo_offset: u64,
        _opts: WriteOptions,
    ) -> WriteAction {
        WriteAction::Write
    }

    // If [`VmoBackedServerOptions::write_tracking`] is enabled, `writes` is set to the batch since
    // last flush or barrier and can be freely modified.
    fn flush(&self, _writes: Option<&mut WriteCache>) {}

    // If [`VmoBackedServerOptions::write_tracking`] is enabled, `writes` is set to the batch since
    // last flush or barrier and can be freely modified.
    fn close(&self, _writes: Option<&mut WriteCache>) {}

    fn trim(&self, _device_block_offset: u64, _block_count: u32) {}
}

pub struct FscryptKeys(BTreeMap<u8, FscryptSoftwareInoLblk32FileCipher>);

impl FscryptKeys {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn evict_key(&mut self, slot: u8) -> Result<(), zx::Status> {
        match self.0.remove(&slot) {
            Some(_) => Ok(()),
            None => Err(zx::Status::INVALID_ARGS),
        }
    }

    pub fn program_key(&mut self, xts_key: &[u8; 64]) -> Result<u8, zx::Status> {
        let unwrapped_key = UnwrappedKey::new(xts_key.to_vec());
        let cipher = FscryptSoftwareInoLblk32FileCipher::new(&unwrapped_key);
        // Find the first keyslot that is not in use and use it.
        for slot in 0..=u8::MAX {
            if self.0.contains_key(&slot) {
                continue;
            }
            self.0.insert(slot, cipher);
            return Ok(slot);
        }
        Err(zx::Status::NO_RESOURCES)
    }

    pub fn get_key(&self, slot: u8) -> Result<&FscryptSoftwareInoLblk32FileCipher, zx::Status> {
        self.0.get(&slot).ok_or(zx::Status::IO)
    }
}

// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Load generator for mutable filesystems which runs random operations.

use anyhow::{Context, Result};
use fidl_fuchsia_io as fio;
use rand::distr::{Distribution, StandardUniform};
use rand::rngs::StdRng;
use rand::seq::IndexedMutRandom as _;
use rand::{Rng, RngExt as _, SeedableRng as _};
use std::sync::atomic::{AtomicU64, Ordering};

// Arbitrary maximum file size just to put a cap on it.
const BLACKOUT_MAX_FILE_SIZE: usize = 1 << 16;

/// Operations that can be performed.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    /// Extend the file with fallocate.
    Allocate,
    /// Write to the file.
    Write,
    /// Truncate the file.
    Truncate,
    /// Close the connection to this file and reopen it. This effectively forces the handle to
    /// flush.
    Reopen,
    /// Delete this file and make a new random one.
    Replace,
}

/// A trait to allow clients to specify their own sample rates (or disable certain operations).
pub trait OpSampler {
    /// Picks a random Op.
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Op;
}

struct File {
    name: String,
    contents: Vec<u8>,
    oid: Option<u64>,
    proxy: Option<fio::FileProxy>,
}

impl std::fmt::Display for File {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "file '{}' (current len: {}) ", self.name, self.contents.len())?;
        if let Some(oid) = self.oid {
            write!(f, "oid: {}", oid)?;
        }
        Ok(())
    }
}

impl Distribution<File> for StandardUniform {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> File {
        static NAME: AtomicU64 = AtomicU64::new(0);
        let size = rng.random_range(1..BLACKOUT_MAX_FILE_SIZE);
        let mut contents = vec![0; size];
        rng.fill(contents.as_mut_slice());
        let name = NAME.fetch_add(1, Ordering::Relaxed);
        File { name: name.to_string(), contents, oid: None, proxy: None }
    }
}

impl File {
    // Create this file on disk in the given directory.
    async fn create(&mut self, dir: &fio::DirectoryProxy) -> Result<()> {
        let file = fuchsia_fs::directory::open_file(
            dir,
            &self.name,
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_MUST_CREATE,
        )
        .await?;
        fuchsia_fs::file::write(&file, &self.contents).await?;
        let (_, attrs) = file
            .get_attributes(fio::NodeAttributesQuery::ID)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        self.oid = attrs.id;
        self.proxy = Some(file);
        Ok(())
    }

    async fn reopen(&mut self, dir: &fio::DirectoryProxy) -> Result<()> {
        let proxy = self.proxy.take().unwrap();
        proxy
            .close()
            .await
            .context("reopen close fidl error")?
            .map_err(zx::Status::err_from_raw)
            .context("reopen close returned error")?;
        self.proxy = Some(
            fuchsia_fs::directory::open_file(
                dir,
                &self.name,
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            )
            .await?,
        );
        Ok(())
    }

    fn proxy(&self) -> &fio::FileProxy {
        self.proxy.as_ref().unwrap()
    }
}

/// Continuously generates load on `root`.  Does not return.
pub async fn generate_load<S: OpSampler>(
    seed: u64,
    op_distribution: &S,
    root: &fio::DirectoryProxy,
) -> Result<()> {
    let mut rng = StdRng::seed_from_u64(seed);

    // Make a set of 16 possible files to mess with.
    let mut files: Vec<File> = (&mut rng).sample_iter(StandardUniform).take(16).collect();
    log::debug!("xx: creating initial files");
    for file in &mut files {
        log::debug!("    creating {}", file);
        file.create(root)
            .await
            .with_context(|| format!("creating file {} during setup", file.name))?;
    }

    log::info!("generating load");
    let mut scan_tick = 0;
    loop {
        if scan_tick >= 20 {
            log::debug!("xx: full scan");
            let mut entries = fuchsia_fs::directory::readdir(root)
                .await?
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>();
            entries.sort();
            let mut expected_entries =
                files.iter().map(|file| file.name.to_string()).collect::<Vec<_>>();
            expected_entries.sort();
            assert_eq!(entries, expected_entries);
            for file in &files {
                log::debug!("    scanning {}", file);
                // Make sure we reset seek, since read and write both move the seek pointer
                let offset = file
                    .proxy()
                    .seek(fio::SeekOrigin::Start, 0)
                    .await
                    .context("scan seek fidl error")?
                    .map_err(zx::Status::err_from_raw)
                    .context("scan seek returned error")?;
                assert_eq!(offset, 0);
                let data = fuchsia_fs::file::read(file.proxy()).await.context("scan read error")?;
                assert_eq!(file.contents.len(), data.len());
                assert_eq!(&file.contents, &data);
            }
            scan_tick = 0;
        } else {
            scan_tick += 1;
        }
        // unwrap: vec is always non-empty so this will never be None.
        let file = files.choose_mut(&mut rng).unwrap();
        match op_distribution.sample(&mut rng) {
            Op::Allocate => {
                // len has to be bigger than zero so make sure there is at least one byte to
                // request.
                let offset = rng.random_range(0..BLACKOUT_MAX_FILE_SIZE - 1);
                let len = rng.random_range(1..BLACKOUT_MAX_FILE_SIZE - offset);
                log::debug!(
                    "op: {}, allocate range: {}..{}, len: {}",
                    file,
                    offset,
                    offset + len,
                    len
                );
                file.proxy()
                    .allocate(offset as u64, len as u64, fio::AllocateMode::empty())
                    .await
                    .context("allocate fidl error")?
                    .map_err(zx::Status::err_from_raw)
                    .context("allocate returned error")?;
                if file.contents.len() < offset + len {
                    file.contents.resize(offset + len, 0);
                }
            }
            Op::Write => {
                // Make sure we are always writing at least one byte.
                let offset = rng.random_range(0..BLACKOUT_MAX_FILE_SIZE - 1);
                let len = rng.random_range(1..std::cmp::min(8192, BLACKOUT_MAX_FILE_SIZE - offset));
                log::debug!(
                    "op: {}, write range: {}..{}, len: {}",
                    file,
                    offset,
                    offset + len,
                    len
                );
                let mut data = vec![0u8; len];
                rng.fill(data.as_mut_slice());
                file.proxy()
                    .write_at(&data, offset as u64)
                    .await
                    .context("write fidl error")?
                    .map_err(zx::Status::err_from_raw)
                    .context("write returned error")?;
                // It's possible we are extending the file with this call, so deal with that
                // here by filling it with zeros and then replacing that with the new content,
                // because any space between the new offset and the old end could be sparse
                // zeros.
                if file.contents.len() < offset + len {
                    file.contents.resize(offset + len, 0);
                }
                file.contents[offset..offset + len].copy_from_slice(&data);
            }
            Op::Truncate => {
                let offset = rng.random_range(0..BLACKOUT_MAX_FILE_SIZE);
                log::debug!("op: {}, truncate offset: {}", file, offset);
                file.proxy()
                    .resize(offset as u64)
                    .await
                    .context("truncate fidl error")?
                    .map_err(zx::Status::err_from_raw)
                    .context("truncate returned error")?;
                file.contents.resize(offset, 0);
            }
            Op::Reopen => {
                log::debug!("op: {}, sync and reopen", file);
                file.reopen(root).await?;
            }
            Op::Replace => {
                log::debug!("op: {}, replace", file);
                file.proxy()
                    .close()
                    .await
                    .context("replace close fidl error")?
                    .map_err(zx::Status::err_from_raw)
                    .context("replace close returned error")?;
                root.unlink(&file.name, &fio::UnlinkOptions::default())
                    .await
                    .context("replace unlink fidl error")?
                    .map_err(zx::Status::err_from_raw)
                    .context("replace unlink returned error")?;
                *file = rng.random();
                log::debug!("    {} is replacement", file);
                file.create(root)
                    .await
                    .with_context(|| format!("creating file {} as a replacement", file.name))?;
            }
        }
    }
}

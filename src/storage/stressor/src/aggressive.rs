// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Starts the 'aggressive' stressor.
//! This variant performs open-read-close, open-append-close, and unlink-open-append-close
//! operations. Note that 'truncate' is not exercised.
//! This version also silently ignores errors such as out of space.

use fidl_fuchsia_io as fio;
use fuchsia_async::MonotonicInstant;
use fuchsia_sync::Mutex;
use rand::distr::weighted::WeightedIndex;
use rand::prelude::*;
use std::fs::File;
use std::io::ErrorKind;
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

struct Inner {
    /// Used to round-robin across NUM_FILES in order.
    file_counter: usize,
    /// Maps from file_counter to file index. This allows us to shuffle the tail
    /// as we round-robin without touching the files that are in the dirent cache.
    file_map: [u64; NUM_FILES],
}

impl Inner {
    /// Returns the next file to use, avoiding the most recent DIRENT_CACHE_LIMIT
    /// files returned but shuffling the remainder.
    pub fn next_file_num(&mut self) -> u64 {
        let mut rng = rand::rng();
        if self.file_counter == 0 {
            log::info!("File counter looped.");
        }
        self.file_map.swap(
            self.file_counter,
            (self.file_counter + rng.random_range(0..(NUM_FILES - DIRENT_CACHE_LIMIT))) % NUM_FILES,
        );
        let ret = self.file_map[self.file_counter];
        self.file_counter = (self.file_counter + 1) % NUM_FILES;
        ret
    }
}

/// Designed to 'exercise' the filesystem and thrash the dirent cache.
/// This cyclically works its way through 10000 files (the dirent cache holds 8000).
///
/// On each iteration it will either read, write or rewrite a file.
/// Unaligned random reads and append-only writes up to 128kB each in length.
/// Write errors causes the file to be truncated to zero bytes.
///
/// The file order is partially permuted each time (only those files which
/// are known not to be in the dirent cache) to reduce cyclic write/delete patterns.
///
/// The truncation of files when we hit the target limit is also randomized to avoid
/// introducing cyclic patterns (n writes, truncate, n writes, truncate, ...).
///
/// Despite these measures, we still expect to see some cyclic behaviour.
/// Each pass has equal probability of read or write. It takes about 3 writes to reach
/// 150kB (the average size before we hit target bytes of 1.5GB) so we can assume
/// n reads, n writes, n/3 truncates in steady state.
///
///   n + n + n/3 = 10000
///   n = 30000 / 7 = 4285
///
/// On average, 4285/3 = ~1500 files are truncated each pass. Assuming an average file
/// size, this is around 220MB or 14%. Best case it will take 7 passes to rewrite all
/// data, but we can assume that most of the data will be rewritten in a few dozen passes.
///
/// Each pass anecdotally takes between 2 and 20 seconds depending on system load,
/// allocation strategy and fragmentation.
pub struct Stressor {
    /// Path prefix to write files to.
    path_prefix: String,
    /// Inner state tracking which files can be accessed next.
    inner: Mutex<Inner>,
    /// Tracks the number of bytes we have allocated.
    bytes_stored: AtomicU64,
    /// The target number of bytes we want the filesystem to consume in steady-state.
    target_bytes: u64,
    op_stats: Mutex<[u64; NUM_OPS]>,
    /// A handle to the directory used to query filesystem info periodically.
    dir: Option<fio::DirectorySynchronousProxy>,
}

const READ: usize = 0;
const WRITE: usize = 1;
const TRUNCATE: usize = 2;
const NUM_OPS: usize = 3;

/// The number of files to cycle through.
/// This must be larger than DIRENT_CACHE_LIMIT to induce cache thrashing.
const NUM_FILES: usize = 4000;

/// See src/storage/fxfs/platform/src/fuchsia/volume.rs.
const DIRENT_CACHE_LIMIT: usize = 2000;

impl Stressor {
    /// Creates a new aggressive stressor that will try to fill the disk until
    /// `target_free_bytes` of free space remains.
    pub fn new(path_prefix: &str, target_free_bytes: u64) -> Arc<Self> {
        let (used_bytes, target_bytes, dir) = if !path_prefix.is_empty() {
            let dir = fio::DirectorySynchronousProxy::new(
                fuchsia_async::Channel::from_channel(
                    fdio::transfer_fd(std::fs::File::open("/data").unwrap()).unwrap().into(),
                )
                .into(),
            );
            let info = dir.query_filesystem(MonotonicInstant::INFINITE.into()).unwrap().1.unwrap();

            log::info!(
                "Aggressive stressor mode targetting {} free bytes on a {} byte volume.",
                target_free_bytes,
                info.total_bytes
            );
            (info.used_bytes, info.total_bytes - target_free_bytes, Some(dir))
        } else {
            log::info!("Aggressive stressor running without path. Targetting 1MB of space.");
            (0, 1 << 20, None)
        };

        Arc::new(Stressor {
            path_prefix: path_prefix.to_owned(),
            inner: Mutex::new(Inner {
                file_counter: 0,
                file_map: std::array::from_fn(|i| i as u64),
            }),
            bytes_stored: AtomicU64::new(used_bytes),
            target_bytes,
            op_stats: Default::default(),
            dir,
        })
    }

    /// Starts the stressor. Loops forever.
    pub fn run(self: &Arc<Self>, num_threads: usize) {
        for _ in 0..num_threads {
            let this = self.clone();
            std::thread::spawn(move || this.worker());
        }
        // Sleep forever, periodically updating bytes stored.
        // This update is racy but close enough for our purposes.
        loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            if let Some(dir) = &self.dir {
                let info =
                    dir.query_filesystem(MonotonicInstant::INFINITE.into()).unwrap().1.unwrap();
                self.bytes_stored.store(info.used_bytes, Ordering::SeqCst);
            }
            log::info!(
                "bytes_stored: {}, target_bytes {}, counts: {:?}",
                self.bytes_stored.load(Ordering::Relaxed),
                self.target_bytes,
                self.op_stats.lock()
            );
        }
    }

    /// The body of the worker loop. Pulled out for testing.
    fn work_unit(&self, rng: &mut rand::rngs::ThreadRng, buf: &mut Vec<u8>) {
        let bytes_stored = self.bytes_stored.load(Ordering::Relaxed);
        // Note that truncate is rare until we exceed 100% target utilization, at which point it
        // becomes as likely as read or write. By the time we hit 105% target utilization,
        // truncates are 2.6x as likely as read or write.
        let weights: [f64; NUM_OPS] = [
            1.0,                                                             /* READ */
            1.0,                                                             /* WRITE */
            f64::powf(bytes_stored as f64 / self.target_bytes as f64, 20.0), /* TRUNCATE */
        ];
        let op = WeightedIndex::new(weights).unwrap().sample(rng);
        let file_num = self.inner.lock().next_file_num();
        let path = format!("{}/{file_num}", self.path_prefix);
        match op {
            READ => match File::options().read(true).write(false).open(&path) {
                Ok(f) => {
                    let file_len = f.metadata().unwrap().len();
                    let read_len = rng.random_range(0..128 * 1024u64);
                    let end = file_len.saturating_sub(read_len);
                    let offset = rng.random_range(0..end + 1);
                    buf.resize(read_len as usize, 0);
                    f.read_at(buf, offset).unwrap();
                }
                Err(e) => match e.kind() {
                    ErrorKind::NotFound => {}
                    error => {
                        log::warn!("Got an open error on READ: {error:?}");
                    }
                },
            },
            WRITE => {
                match File::options().create(true).read(true).write(true).open(&path) {
                    Ok(f) => {
                        let file_len = f.metadata().unwrap().len();
                        buf.resize(rng.random_range(0..128 * 1024), 1);
                        match f.write_at(&buf, file_len) {
                            Ok(bytes) => {
                                self.bytes_stored.fetch_add(bytes as u64, Ordering::Relaxed);
                            }
                            Err(_) => {
                                // When a write fails due to space, we truncate.
                                // In this way we fill up the disk then start fragmenting it.
                                // Until #![feature(io_error_more)] is stable, we have
                                // a catch-all here.
                                let _ = f.set_len(0).unwrap();
                                // Metadata (layer files and such) can take up megabytes of
                                // space that is not tracked in `self.bytes_stored`.
                                // If we hit this code path then we've blown through
                                // available space but not hit our 'bytes_stored' limit.
                                //
                                // Writes are relatively small compared to the potential
                                // layer file size so even though it's a bit racy, we will
                                // accept the potential inaccuracy of concurrent writes
                                // and adjust bytes_stored here based on the filesystem's
                                // internal tally.
                                if let Some(dir) = &self.dir {
                                    let info = dir
                                        .query_filesystem(MonotonicInstant::INFINITE.into())
                                        .unwrap()
                                        .1
                                        .unwrap();
                                    log::info!("Correcting bytes_stored. {info:?}");
                                    self.bytes_stored.store(info.used_bytes, Ordering::SeqCst);
                                }
                            }
                        };
                    }
                    // Unfortunately ErrorKind::StorageFull is unstable so
                    // we rely on a catch-all here and assume write errors are
                    // due to disk being full. This happens often so we don't log it.
                    Err(_) => {}
                }
            }
            TRUNCATE => match File::options().write(true).open(&path) {
                Ok(f) => {
                    let file_len = f.metadata().unwrap().len();
                    self.bytes_stored.fetch_sub(file_len, Ordering::Relaxed);
                    let _ = f.set_len(0).unwrap();
                }
                Err(_) => {}
            },
            _ => unreachable!(),
        }
        self.op_stats.lock()[op] += 1;
    }

    /// Worker thread function.
    fn worker(&self) {
        let mut rng = rand::rng();
        let mut buf = Vec::new();
        loop {
            self.work_unit(&mut rng, &mut buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stressor() {
        let stressor = Stressor::new("", 10);
        let mut rng = rand::rng();
        let mut buf = Vec::new();
        stressor.work_unit(&mut rng, &mut buf);
        assert_eq!(stressor.op_stats.lock().iter().sum::<u64>(), 1);
        stressor.work_unit(&mut rng, &mut buf);
        assert_eq!(stressor.op_stats.lock().iter().sum::<u64>(), 2);
    }
}

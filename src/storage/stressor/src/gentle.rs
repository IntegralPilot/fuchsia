// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This variant opens, closes, creates, deletes, reads, writes and truncates small files.
//! Reads and writes are at offsets less than 100k and lengths less than 10k.
//! Open file count is kept probabilistically low to avoid using too much disk.
//! Panics if operations fail.

use fuchsia_sync::{Mutex, RwLock};
use rand::RngExt as _;
use rand::distr::Distribution as _;
use rand::distr::weighted::WeightedIndex;
use rand::seq::IndexedRandom as _;
use std::fs::File;
use std::io::ErrorKind;
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Stressor {
    name_counter: AtomicU64,
    all_files: RwLock<Vec<Arc<FileState>>>,
    open_files: RwLock<Vec<Arc<std::fs::File>>>,
    op_stats: Mutex<[u64; NUM_OPS]>,
}

#[derive(Eq, PartialEq)]
struct FileState {
    path: String,
}

const OPEN_FILE: usize = 0;
const CLOSE_FILE: usize = 1;
const CREATE_FILE: usize = 2;
const DELETE_FILE: usize = 3;
const READ: usize = 4;
const WRITE: usize = 5;
const TRUNCATE: usize = 6;

const NUM_OPS: usize = 7;

impl Stressor {
    pub fn new() -> Arc<Self> {
        // Find any existing files.
        let mut files = Vec::new();
        let mut max_counter = 0;
        for entry in std::fs::read_dir("/data").unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if let Some(counter) = path
                .strip_prefix("/data/")
                .ok()
                .and_then(|p| p.to_str())
                .and_then(|s| s.parse().ok())
            {
                if counter > max_counter {
                    max_counter = counter;
                }
            }
            files.push(Arc::new(FileState { path: path.to_str().unwrap().to_string() }));
        }

        Arc::new(Stressor {
            name_counter: AtomicU64::new(max_counter + 1),
            all_files: RwLock::new(files),
            ..Stressor::default()
        })
    }

    /// Weights used to decide which operation to probabilistically favour.
    fn get_weights(&self) -> [f64; NUM_OPS] {
        let all_file_count = self.all_files.read().len() as f64;
        let open_file_count = self.open_files.read().len() as f64;
        let if_open = |x| if open_file_count > 0.0 { x } else { 0.0 };
        [
            /* OPEN_FILE: */ 1.0 / (open_file_count + 1.0) * all_file_count,
            /* CLOSE_FILE: */ open_file_count * 0.1,
            /* CREATE_FILE: */ 2.0 / (all_file_count + 1.0),
            /* DELETE_FILE: */ all_file_count * 0.005,
            /* READ: */ if_open(1000.0),
            /* WRITE: */ if_open(1000.0),
            /* TRUNCATE: */ if_open(1000.0),
        ]
    }

    /// Starts the stressor. Loops forever.
    pub fn run(self: &Arc<Self>, num_threads: usize) {
        log::info!(
            "Running stressor, found {} files, counter: {}",
            self.all_files.read().len(),
            self.name_counter.load(Ordering::Relaxed),
        );
        for _ in 0..num_threads {
            let this = self.clone();
            std::thread::spawn(move || this.worker());
        }

        loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            let all_file_count = self.all_files.read().len();
            let open_file_count = self.open_files.read().len();
            log::info!(
                "{} files, {} open, weights: {:?}, counts: {:?}",
                all_file_count,
                open_file_count,
                self.get_weights(),
                self.op_stats.lock()
            );
        }
    }

    /// Worker thread function.
    fn worker(&self) {
        let mut rng = rand::rng();
        let mut buf = Vec::new();
        loop {
            let weights: [f64; NUM_OPS] = self.get_weights();
            let op = WeightedIndex::new(weights).unwrap().sample(&mut rng);
            match op {
                OPEN_FILE => {
                    let Some(file) = self.all_files.read().choose(&mut rng).cloned() else {
                        continue;
                    };
                    match File::options().read(true).write(true).open(&file.path) {
                        Ok(f) => self.open_files.write().push(Arc::new(f)),
                        Err(e) => match e.kind() {
                            ErrorKind::NotFound => {}
                            e => {
                                panic!("open file failed with error {e:?}");
                            }
                        },
                    }
                }
                CLOSE_FILE => {
                    let _file = {
                        let mut open_files = self.open_files.write();
                        let num = open_files.len();
                        if num == 0 {
                            continue;
                        }
                        open_files.remove(rng.random_range(0..num))
                    };
                }
                CREATE_FILE => {
                    let path =
                        format!("/data/{}", self.name_counter.fetch_add(1, Ordering::Relaxed));
                    match File::options().create(true).read(true).write(true).open(&path) {
                        Ok(file) => {
                            let file_state = Arc::new(FileState { path });
                            self.all_files.write().push(file_state);
                            self.open_files.write().push(Arc::new(file));
                        }
                        Err(error) => {
                            panic!("Failed to create file: {error:?}");
                        }
                    };
                }
                DELETE_FILE => {
                    let file = {
                        let mut all_files = self.all_files.write();
                        if all_files.is_empty() {
                            continue;
                        }
                        let num = all_files.len();
                        all_files.remove(rng.random_range(0..num))
                    };
                    std::fs::remove_file(&file.path).unwrap();
                }
                READ => {
                    let Some(file) = self.open_files.read().choose(&mut rng).cloned() else {
                        continue;
                    };
                    let read_offset = rng.random_range(0..100_000) as u64;
                    let read_len = (-rng.random::<f64>().ln() * 100_000.0) as u64;
                    buf.resize(read_len as usize, 1);
                    file.read_at(&mut buf, read_offset).unwrap();
                }
                WRITE => {
                    let Some(file) = self.open_files.read().choose(&mut rng).cloned() else {
                        continue;
                    };
                    let write_len = (-rng.random::<f64>().ln() * 10_000.0) as u64;
                    let write_offset = rng.random_range(0..100_000) as u64;
                    buf.resize(write_len as usize, 1);
                    file.write_at(&buf, write_offset).unwrap();
                }
                TRUNCATE => {
                    let Some(file) = self.open_files.read().choose(&mut rng).cloned() else {
                        continue;
                    };
                    file.set_len(rng.random_range(0..100_000)).unwrap();
                }
                _ => unreachable!(),
            }
            self.op_stats.lock()[op] += 1;
        }
    }
}

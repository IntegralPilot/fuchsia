// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::FatFs;
use crate::directory::FatDirectory;
use crate::node::{Closer, FatNode, Node};
use crate::types::Disk;
use anyhow::Error;
use fidl_fuchsia_io as fio;
use futures::future::BoxFuture;
use futures::prelude::*;
use scopeguard::defer;
use std::any::Any;
use std::io::Cursor;
use std::sync::Arc;
use vfs::directory::dirents_sink::{AppendResult, Sealed, Sink};
use vfs::directory::entry::EntryInfo;
use vfs::directory::entry_container::Directory;
use vfs::directory::traversal_position::TraversalPosition;
use vfs::file::{File, FileIo};
use vfs::node::Node as _;
use zx::Status;

impl Disk for std::io::Cursor<Box<[u8]>> {
    fn is_present(&self) -> bool {
        true
    }
}

fn fuzz_node(fs: &FatFs, node: FatNode, depth: u32) -> BoxFuture<'_, Result<(), Status>> {
    async move {
        if depth > 5 {
            // It's possible for a FAT filesystem to contain cycles while technically being legal.
            return Ok(());
        }
        match node {
            FatNode::File(file) => {
                let mut buffer = vec![0u8; 2084];
                let _ = file.read_at(0, &mut buffer).await;
                let _ = file.write_at(256, "qwerty".as_bytes()).await;
                let _ = file.get_size().await;
            }
            FatNode::Dir(dir) => {
                let sink = FuzzSink::new(dir.clone(), depth);
                let (pos, sealed): (TraversalPosition, Box<dyn Sealed>) =
                    dir.read_dirents(&TraversalPosition::Start, Box::new(sink)).await?;
                assert_eq!(pos, TraversalPosition::End);
                let sink = sealed.open().downcast::<FuzzSink>().unwrap();
                sink.walk(fs).await;
            }
        };

        Ok(())
    }
    .boxed()
}

struct FuzzSink {
    entries: Vec<String>,
    dir: Arc<FatDirectory>,
    depth: u32,
}

impl FuzzSink {
    fn new(dir: Arc<FatDirectory>, depth: u32) -> Self {
        Self { entries: Vec::new(), dir, depth }
    }

    async fn walk(&self, fs: &FatFs) {
        for name in self.entries.iter() {
            let mut closer = Closer::new();
            let entry = match self.dir.open_child(
                name,
                fio::OpenFlags::RIGHT_READABLE | fio::OpenFlags::RIGHT_WRITABLE,
                &mut closer,
            ) {
                Ok(entry) => entry,
                Err(_) => continue,
            };

            let _ = fuzz_node(fs, entry, self.depth + 1).await;
        }
    }
}

impl Sink for FuzzSink {
    fn append(mut self: Box<Self>, _entry: &EntryInfo, name: &str) -> AppendResult {
        if name != ".." && name != "." {
            self.entries.push(name.to_owned());
        }
        AppendResult::Ok(self)
    }

    fn seal(self: Box<Self>) -> Box<dyn Sealed> {
        self
    }
}

impl Sealed for FuzzSink {
    fn open(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

async fn do_fuzz(disk: Cursor<Box<[u8]>>) -> Result<(), Error> {
    let fs = FatFs::new(Box::new(disk), vfs::execution_scope::ExecutionScope::new())?;
    let root: Arc<FatDirectory> = fs.get_fatfs_root();

    root.open_ref().unwrap();
    let _ = fuzz_node(&fs, FatNode::Dir(root.clone()), 0).await;
    defer! { root.close() };

    Ok(())
}

pub fn fuzz_fatfs(fs: &[u8]) {
    let mut executor = fuchsia_async::TestExecutor::new();
    executor.run_singlethreaded(async {
        let mut vec = fs.to_vec();
        // Make sure the "disk" is always a length that's a multiple of 512.
        let rounded = ((vec.len() / 512) + 1) * 512;
        // Add an additional 4MiB to the disk size so fatfs has space to write to.
        vec.resize(rounded + 4 * 1024 * 1024, 0);
        let cursor = std::io::Cursor::new(vec.into_boxed_slice());

        let _ = do_fuzz(cursor).await;
    });
}

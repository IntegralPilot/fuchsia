// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use fuchsia_fs::PERM_READABLE;
use fuchsia_fs::directory::{
    DirEntry, DirentKind, open_file_async, open_in_namespace, readdir_recursive,
};
use futures::StreamExt as _;

/// Pages in and retains all of the blobs in the pkg directory. This is done to avoid page faulting
/// on a blob during a benchmark.
pub struct BlobLoader {
    vmos: Vec<(zx::Vmo, u64)>,
}

impl BlobLoader {
    pub async fn load_blobs() -> Self {
        let vmos = collect_pkg_vmos().await;
        apply_vmo_op(&vmos, zx::VmoOp::ALWAYS_NEED);
        Self { vmos }
    }
}

impl Drop for BlobLoader {
    fn drop(&mut self) {
        apply_vmo_op(&self.vmos, zx::VmoOp::DONT_NEED);
    }
}

async fn collect_pkg_vmos() -> Vec<(zx::Vmo, u64)> {
    let dir = open_in_namespace("/pkg", PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY)
        .expect("Failed to open /pkg directory");
    let mut vmos = Vec::new();
    let mut entries = readdir_recursive(&dir, None);
    while let Some(entry) = entries.next().await {
        let DirEntry { name, kind } = entry.unwrap();
        if kind != DirentKind::File {
            continue;
        }
        let file = open_file_async(&dir, &name, PERM_READABLE).unwrap();
        let vmo =
            file.get_backing_memory(fio::VmoFlags::READ).await.unwrap().map_err(zx::ok).unwrap();
        let size = vmo.get_size().unwrap();
        if size > 0 {
            vmos.push((vmo, size));
        }
    }
    vmos
}

fn apply_vmo_op(vmos: &[(zx::Vmo, u64)], op: zx::VmoOp) {
    for (vmo, size) in vmos {
        vmo.op_range(op, 0, *size).unwrap();
    }
}

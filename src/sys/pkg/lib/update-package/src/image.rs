// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;
use zx_status::Status;

#[cfg(target_os = "fuchsia")]
use {fidl_fuchsia_io as fio, fidl_fuchsia_mem as fmem};

/// An error encountered while opening an image.
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum OpenImageError {
    #[error("while opening the file path {path:?}")]
    OpenPath {
        path: String,
        #[source]
        err: fuchsia_fs::node::OpenError,
    },

    #[error("while calling get_backing_memory for {path:?}")]
    FidlGetBackingMemory {
        path: String,
        #[source]
        err: fidl::Error,
    },

    #[error("while obtaining vmo of file for {path:?}: {status}")]
    GetBackingMemory { path: String, status: Status },

    #[error("while converting vmo to a resizable vmo for {path:?}: {status}")]
    CloneBuffer { path: String, status: Status },
}

#[cfg(target_os = "fuchsia")]
/// Opens the given `path` as a resizable VMO buffer and returns the buffer on success.
pub(crate) async fn open_from_path(
    proxy: &fio::DirectoryProxy,
    path: &str,
) -> Result<fmem::Buffer, OpenImageError> {
    let file = fuchsia_fs::directory::open_file(proxy, path, fio::PERM_READABLE)
        .await
        .map_err(|err| OpenImageError::OpenPath { path: path.to_string(), err })?;

    let vmo = file
        .get_backing_memory(fio::VmoFlags::READ)
        .await
        .map_err(|err| OpenImageError::FidlGetBackingMemory { path: path.to_string(), err })?
        .map_err(Status::err_from_raw)
        .map_err(|status| OpenImageError::GetBackingMemory { path: path.to_string(), status })?;

    let size = vmo
        .get_content_size()
        .map_err(|status| OpenImageError::GetBackingMemory { path: path.to_string(), status })?;

    // The paver service requires VMOs that are resizable, and blobfs does not give out resizable
    // VMOs. Fortunately, a copy-on-write child clone of the vmo can be made resizable.
    let vmo = vmo
        .create_child(
            zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE | zx::VmoChildOptions::RESIZABLE,
            0,
            size,
        )
        .map_err(|status| OpenImageError::CloneBuffer { path: path.to_string(), status })?;

    Ok(fmem::Buffer { vmo, size })
}

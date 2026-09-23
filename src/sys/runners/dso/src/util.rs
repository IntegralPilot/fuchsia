// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::StartError;
use fidl_fuchsia_io as fio;
use fuchsia_component::directory;
use namespace::Namespace;

pub(super) async fn open_pkg_path(
    ns: &Namespace,
    path: &str,
) -> Result<fio::DirectoryProxy, StartError> {
    let pkg = ns.get(&"/pkg".parse().unwrap()).ok_or(StartError::InvalidNamespace)?;
    directory::open_directory_async(pkg, path, fio::RX_STAR_DIR)
        .map_err(|err| StartError::OpenPackagePathFidl { path: path.into(), err })
}

pub(super) async fn get_pkg_file_vmo(ns: &Namespace, path: &str) -> Result<zx::Vmo, StartError> {
    let pkg = ns.get(&"/pkg".parse().unwrap()).ok_or(StartError::InvalidNamespace)?;
    let file = directory::open_file_async(pkg, path, fio::RX_STAR_DIR)
        .map_err(|err| StartError::OpenDsoFidl { path: path.into(), err })?;
    file.get_backing_memory(
        fio::VmoFlags::READ | fio::VmoFlags::EXECUTE | fio::VmoFlags::PRIVATE_CLONE,
    )
    .await
    .map_err(|err| StartError::OpenDsoFidl { path: path.into(), err: err.into() })?
    .map_err(|err| StartError::OpenDso { path: path.into(), err: fidl::Status::err_from_raw(err) })
}

pub(super) fn basename(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, filename)) => filename,
        None => path,
    }
}

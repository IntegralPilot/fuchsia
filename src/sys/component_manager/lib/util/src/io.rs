// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use fuchsia_fs::directory::clone;
use std::path::PathBuf;

// TODO(https://fxbug.dev/42176573): We should probably preserve the original error messages
// instead of dropping them.
pub fn clone_dir(dir: Option<&fio::DirectoryProxy>) -> Option<fio::DirectoryProxy> {
    dir.and_then(|d| clone(d).ok())
}

/// Fallible conversion from [`PathBuf`] to [`vfs::path::Path`].
///
/// TODO(https://fxbug.dev/331429249): This should be removed by replacing
/// [`PathBuf`] with other more Fuchsia-y paths.
pub fn path_buf_to_vfs_path(path: PathBuf) -> Option<vfs::path::Path> {
    let path = path.to_str()?;
    if path.is_empty() {
        Some(vfs::path::Path::dot())
    } else {
        vfs::path::Path::validate_and_split(path).ok()
    }
}

// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Crate for directory operations.
//
// TODO(https://fxbug.dev/42083023): These operations can be merged into `fuchsia-fs` if Rust FIDL bindings
// support making one-way calls on a client endpoint without turning it into a proxy.

#![deny(missing_docs)]

use anyhow::{Context, Error};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_io as fio;
use std::sync::Arc;

/// A trait for opening filesystem nodes.
pub trait Directory {
    /// Open a node relative to the directory.
    fn open(&self, path: &str, flags: fio::Flags, server_end: zx::Channel) -> Result<(), Error>;
}

impl Directory for fio::DirectoryProxy {
    fn open(&self, path: &str, flags: fio::Flags, server_end: zx::Channel) -> Result<(), Error> {
        #[cfg(fuchsia_api_level_at_least = "27")]
        let () = self.open(path, flags, &fio::Options::default(), server_end.into())?;
        #[cfg(not(fuchsia_api_level_at_least = "27"))]
        let () = self.open3(path, flags, &fio::Options::default(), server_end.into())?;
        Ok(())
    }
}

impl Directory for fio::DirectorySynchronousProxy {
    fn open(&self, path: &str, flags: fio::Flags, server_end: zx::Channel) -> Result<(), Error> {
        #[cfg(fuchsia_api_level_at_least = "27")]
        let () = self.open(path, flags, &fio::Options::default(), server_end.into())?;
        #[cfg(not(fuchsia_api_level_at_least = "27"))]
        let () = self.open3(path, flags, &fio::Options::default(), server_end.into())?;
        Ok(())
    }
}

impl Directory for ClientEnd<fio::DirectoryMarker> {
    fn open(&self, path: &str, flags: fio::Flags, server_end: zx::Channel) -> Result<(), Error> {
        let raw_handle = self.channel().as_handle_ref().raw_handle();
        // Safety: we call forget on objects that reference `raw_handle` leaving it usable.
        unsafe {
            let borrowed: zx::Channel = zx::NullableHandle::from_raw(raw_handle).into();
            let proxy = fio::DirectorySynchronousProxy::new(borrowed);
            #[cfg(fuchsia_api_level_at_least = "27")]
            let res = proxy.open(path, flags, &fio::Options::default(), server_end.into());
            #[cfg(not(fuchsia_api_level_at_least = "27"))]
            let res = proxy.open3(path, flags, &fio::Options::default(), server_end.into());
            std::mem::forget(proxy.into_channel());
            res?;
        }
        Ok(())
    }
}

/// A trait for types that can vend out a [`Directory`] reference.
///
/// A new trait is needed because both `DirectoryProxy` and `AsRef` are external types.
/// As a result, implementing `AsRef<&dyn Directory>` for `DirectoryProxy` is not allowed
/// under coherence rules.
pub trait AsRefDirectory {
    /// Get a [`Directory`] reference.
    fn as_ref_directory(&self) -> &dyn Directory;
}

impl AsRefDirectory for fio::DirectoryProxy {
    fn as_ref_directory(&self) -> &dyn Directory {
        self
    }
}

impl AsRefDirectory for fio::DirectorySynchronousProxy {
    fn as_ref_directory(&self) -> &dyn Directory {
        self
    }
}

impl AsRefDirectory for ClientEnd<fio::DirectoryMarker> {
    fn as_ref_directory(&self) -> &dyn Directory {
        self
    }
}

impl<T: Directory> AsRefDirectory for Box<T> {
    fn as_ref_directory(&self) -> &dyn Directory {
        &**self
    }
}

impl<T: Directory> AsRefDirectory for Arc<T> {
    fn as_ref_directory(&self) -> &dyn Directory {
        &**self
    }
}

impl<T: Directory> AsRefDirectory for &T {
    fn as_ref_directory(&self) -> &dyn Directory {
        *self
    }
}

/// Opens the given `path` from the given `parent` directory as a [`DirectoryProxy`] asynchronously.
pub fn open_directory_async(
    parent: &impl AsRefDirectory,
    path: &str,
    rights: fio::Rights,
) -> Result<fio::DirectoryProxy, Error> {
    let (dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();

    let flags = fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::from_bits_truncate(rights.bits());
    let () = parent
        .as_ref_directory()
        .open(path, flags, server_end.into_channel())
        .context("opening directory without describe")?;

    Ok(dir)
}

/// Opens the given `path` from the given `parent` directory as a [`FileProxy`] asynchronously.
pub fn open_file_async(
    parent: &impl AsRefDirectory,
    path: &str,
    rights: fio::Rights,
) -> Result<fio::FileProxy, Error> {
    let (file, server_end) = fidl::endpoints::create_proxy::<fio::FileMarker>();

    let flags = fio::Flags::PROTOCOL_FILE | fio::Flags::from_bits_truncate(rights.bits());
    let () = parent
        .as_ref_directory()
        .open(path, flags, server_end.into_channel())
        .context("opening file without describe")?;

    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use vfs::directory::immutable::simple;
    use vfs::execution_scope::ExecutionScope;
    use vfs::file::vmo::read_only;
    use vfs::pseudo_directory;

    #[fasync::run_singlethreaded(test)]
    async fn open_directory_async_real() {
        let dir = pseudo_directory! {
            "dir" => simple(),
        };
        let dir = vfs::directory::serve_read_only(dir, ExecutionScope::new());
        let dir = open_directory_async(&dir, "dir", fio::Rights::empty()).unwrap();
        fuchsia_fs::directory::close(dir).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_directory_async_fake() {
        let dir = pseudo_directory! {
            "dir" => simple(),
        };
        let dir = vfs::directory::serve_read_only(dir, ExecutionScope::new());
        let dir = open_directory_async(&dir, "fake", fio::Rights::empty()).unwrap();
        // The open error is not detected until the proxy is interacted with.
        assert_matches!(fuchsia_fs::directory::close(dir).await, Err(_));
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_file_async_real() {
        let dir = pseudo_directory! {
            "file" => read_only("read_only"),
        };
        let dir = vfs::directory::serve_read_only(dir, ExecutionScope::new());
        let file = open_file_async(&dir, "file", fio::Rights::READ_BYTES).unwrap();
        fuchsia_fs::file::close(file).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_file_async_fake() {
        let dir = pseudo_directory! {
            "file" => read_only("read_only"),
        };
        let dir = vfs::directory::serve_read_only(dir, ExecutionScope::new());
        let fake = open_file_async(&dir, "fake", fio::Rights::READ_BYTES).unwrap();
        // The open error is not detected until the proxy is interacted with.
        assert_matches!(fuchsia_fs::file::close(fake).await, Err(_));
    }
}

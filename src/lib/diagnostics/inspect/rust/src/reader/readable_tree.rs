// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides implementations for common structs that can be read in its entirety. These are structs
//! that can be interpreted using the `fuchsia.inspect.Tree` protocol.

use crate::Inspector;
use crate::reader::ReaderError;
use async_trait::async_trait;

#[cfg(target_os = "fuchsia")]
pub type SnapshotSource = zx::Vmo;

#[cfg(not(target_os = "fuchsia"))]
pub type SnapshotSource = Vec<u8>;

/// Trait implemented by structs that can provide inspect data and their lazy links.
#[async_trait]
pub trait ReadableTree: Sized {
    /// Returns the lazy links names.
    async fn tree_names(&self) -> Result<Vec<String>, ReaderError>;

    /// Returns the vmo of the current root node.
    async fn vmo(&self) -> Result<SnapshotSource, ReaderError>;

    /// Loads the lazy link of the given `name`.
    async fn read_tree(&self, name: &str) -> Result<Self, ReaderError>;
}

#[async_trait]
impl ReadableTree for Inspector {
    async fn vmo(&self) -> Result<SnapshotSource, ReaderError> {
        self.duplicate_vmo().ok_or(ReaderError::DuplicateVmo)
    }

    async fn tree_names(&self) -> Result<Vec<String>, ReaderError> {
        match self.state() {
            // A no-op inspector.
            None => Ok(vec![]),
            Some(state) => {
                let state = state.try_lock().map_err(ReaderError::FailedToLockState)?;
                let names =
                    state.callbacks().keys().map(|k| k.to_string()).collect::<Vec<String>>();
                Ok(names)
            }
        }
    }

    async fn read_tree(&self, name: &str) -> Result<Self, ReaderError> {
        let cloned_callbacks = self.state().and_then(|state| match state.try_lock() {
            Err(_) => None,
            Ok(state) => state.callbacks().get(name).cloned(),
        });
        let result = cloned_callbacks.map(|value| value());
        match result {
            Some(cb_result) => cb_result.await.map_err(ReaderError::LazyCallback),
            None => return Err(ReaderError::FailedToLoadTree(name.to_string())),
        }
    }
}

#[cfg(target_os = "fuchsia")]
#[async_trait]
impl ReadableTree for fidl_fuchsia_inspect::TreeProxy {
    async fn vmo(&self) -> Result<zx::Vmo, ReaderError> {
        let tree_content = self.get_content().await.map_err(|e| ReaderError::Fidl(e.into()))?;
        tree_content.buffer.map(|b| b.vmo).ok_or(ReaderError::FetchVmo)
    }

    async fn tree_names(&self) -> Result<Vec<String>, ReaderError> {
        let (name_iterator, server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_inspect::TreeNameIteratorMarker>();
        self.list_child_names(server_end).map_err(|e| ReaderError::Fidl(e.into()))?;
        let mut names = vec![];
        loop {
            let subset_names =
                name_iterator.get_next().await.map_err(|e| ReaderError::Fidl(e.into()))?;
            if subset_names.is_empty() {
                return Ok(names);
            }
            names.extend(subset_names);
        }
    }

    async fn read_tree(&self, name: &str) -> Result<Self, ReaderError> {
        let (child_tree, server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_inspect::TreeMarker>();
        self.open_child(name, server_end).map_err(|e| ReaderError::Fidl(e.into()))?;
        Ok(child_tree)
    }
}

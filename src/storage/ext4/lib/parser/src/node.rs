// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use vfs::directory::entry::DirectoryEntry;

use crate::directory::ExtDirectory;
use crate::file::ExtFile;
use crate::symlink::ExtSymlink;
use std::sync::Arc;

/// Represents an ext4 filesystem node which may be either a directory or a file. This holds a
/// strong reference to the contained node.
#[derive(Clone, Debug)]
pub enum ExtNode {
    Dir(Arc<ExtDirectory>),
    File(Arc<ExtFile>),
    Symlink(Arc<ExtSymlink>),
}

impl ExtNode {
    pub fn as_entry(&self) -> &dyn DirectoryEntry {
        match self {
            Self::Dir(dir) => dir.as_ref(),
            Self::File(file) => file.as_ref(),
            Self::Symlink(symlink) => symlink.as_ref(),
        }
    }
}

impl From<ExtDirectory> for ExtNode {
    fn from(value: ExtDirectory) -> Self {
        Self::Dir(Arc::new(value))
    }
}

impl From<ExtFile> for ExtNode {
    fn from(value: ExtFile) -> Self {
        Self::File(Arc::new(value))
    }
}

impl From<ExtSymlink> for ExtNode {
    fn from(value: ExtSymlink) -> Self {
        Self::Symlink(Arc::new(value))
    }
}

impl From<Arc<ExtDirectory>> for ExtNode {
    fn from(value: Arc<ExtDirectory>) -> Self {
        Self::Dir(value)
    }
}

impl From<Arc<ExtFile>> for ExtNode {
    fn from(value: Arc<ExtFile>) -> Self {
        Self::File(value)
    }
}

impl From<Arc<ExtSymlink>> for ExtNode {
    fn from(value: Arc<ExtSymlink>) -> Self {
        Self::Symlink(value)
    }
}

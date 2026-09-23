// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod lazy_pseudo_directory;

pub use crate::lazy_pseudo_directory::{
    LazyPseudoDirectory, LazyPseudoDirectoryAsync, LazyPseudoDirectoryState, ToPseudoDirectory,
    ToPseudoDirectoryAsync,
};
pub use vfs::directory::simple::Simple as PseudoDirectory;
pub use vfs::file::vmo::VmoFile as PseudoFile;

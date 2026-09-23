// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! EROFS Component Library.
//!
//! This crate provides the filesystem entity implementations (directories, files,
//! and pager-backed storage structures) used to host and serve EROFS images.

pub mod directory;
pub mod file;
pub mod pager;
pub mod symlink;
pub mod volume;

pub use directory::ErofsDirectory;
pub use file::ErofsFile;
pub use pager::ErofsPager;
pub use symlink::ErofsSymlink;
pub use volume::ErofsVolume;

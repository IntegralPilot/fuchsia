// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Library for constructing and parsing `AssemblyContainer`s, which hold
//! various artifacts. An `AssemblyContainer` is defined by a config struct that
//! may hold many Utf8PathBufs. This config can get written to a directory on
//! disk, and all the paths will get copied into the directory in a structured
//! manner so that the container is hermetic and can be copied between machines.
//!
//! `WalkPaths` is used to find all the paths inside the config in order to
//! copy the files into a hermetic directory and transform them from absolute
//! to relative.
//!
//! For every field that may contain paths, you must add #[walk_paths] and
//! derive `WalkPaths` on the type.
//!
//! Here is an example:
//! ```
//! use assembly_container::{assembly_container, AssemblyContainer, WalkPaths};
//! use camino::Utf8PathBuf;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize, Serialize, WalkPaths)]
//! #[assembly_container(my_config.json)]
//! struct MyConfig {
//!    #[walk_paths]
//!    inner: Inner,
//! }
//!
//! #[derive(Deserialize, Serialize, WalkPaths)]
//! struct Inner {
//!     #[walk_paths]
//!     path: Utf8PathBuf,
//! }
//!
//! fn main() {
//!    let c = MyConfig {
//!        inner: Inner { path: "path/to/file.txt" },
//!    };
//!    c.write_to_dir("my_dir", Some("depfile")).unwrap();
//! }
//! ```
//!
//! This will generate a directory with the following structure.
//! ```
//! my_dir/
//!   |- my_config.json
//!   |- inner/
//!        |- path/
//!             |- file.txt
//! ```

mod assembly_container;
mod merge;

pub use assembly_container::{
    AssemblyContainer, DirectoryPathBuf, FileType, WalkPaths, WalkPathsFn,
};
pub use assembly_container_macro::{WalkPaths, assembly_container};

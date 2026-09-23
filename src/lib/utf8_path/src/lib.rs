// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Utility methods for creating and manipulating UTF-8 paths.

mod path_to_string;

mod paths;

pub use path_to_string::PathToStringExt;
pub use paths::{
    normalize_path, path_relative_from, path_relative_from_current_dir, path_relative_from_file,
    resolve_path, resolve_path_from_file,
};

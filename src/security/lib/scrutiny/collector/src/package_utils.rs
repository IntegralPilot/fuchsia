// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::path::PathBuf;

const CF_V2_EXT: &str = "cm";
const CF_CONFIG_VALUE_EXT: &str = "cvf";

/// Determine whether a path in a package appears to be a Component Framework V2 manifest file. This predicate
/// appearance is determined by whether the file appears in the `meta/` directory, and ends with a
/// `.cm` extension.
pub(crate) fn is_cf_v2_manifest(path_buf: &PathBuf) -> bool {
    is_meta_with_extension(path_buf, CF_V2_EXT)
}

pub(crate) fn is_cf_v2_config_values(path_buf: &PathBuf) -> bool {
    is_meta_with_extension(path_buf, CF_CONFIG_VALUE_EXT)
}

/// Determine whether `path_buf` is a metadata file (in the Fuchsia `meta.far` sense) that ends with
/// `extension`. Note that `extension` does not include a leading dot, but looks for an extension
/// form with a leading dot.
///
/// Examples:
/// ```
///     ("meta/foo.cm", "cm") => true
///     ("data/foo.cm", "cm") => false
///     ("meta/foo.cm", "json") => false
///     ("meta/foocm", "cm") => false
///     ("meta/foo.cm", ".cm") => false
/// ```
fn is_meta_with_extension(path_buf: &PathBuf, extension: &str) -> bool {
    path_buf.starts_with("meta/")
        && path_buf.extension().map(|xtn| xtn.to_str().map(|xtn| xtn == extension) == Some(true))
            == Some(true)
}

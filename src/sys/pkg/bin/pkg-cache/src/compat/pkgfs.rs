// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;

mod validation;

/// Make the pkgfs compatibility directory, which has the following structure:
///   ./ctl/validation/missing
pub fn make_dir(
    base_packages: Arc<crate::BaseIndex>,
    blobfs: blobfs::Client,
) -> Arc<dyn vfs::directory::entry::DirectoryEntry> {
    vfs::pseudo_directory! {
        "ctl" => vfs::pseudo_directory! {
            "validation" => validation::Validation::new(
                blobfs,
                base_packages,
            )
        },
    }
}

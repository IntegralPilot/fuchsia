// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use futures::stream::StreamExt as _;

/// Exposes the component's package directory.
/// fake_dependencies.rs uses this to write the to-be-resolved subpackage to the blobfs it gives to
/// the base-resolver under test.

#[fuchsia::main]
async fn main() {
    let mut fs = fuchsia_component::server::ServiceFs::new_local();
    fs.add_remote(
        "pkg",
        fuchsia_fs::directory::open_in_namespace("/pkg", fio::PERM_READABLE)
            .expect("opening /pkg dir"),
    );
    fs.take_and_serve_directory_handle().expect("failed to take startup handle");
    let () = fs.collect().await;
}
